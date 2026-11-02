# Phrases, variation and lines: proposal

**Status:** accepted 2026-11-02, when the owner took every recommended
[decision](#decisions). Every part has landed, and "After the Rain" version 5 passed the listening rounds; [Results](#results) records them. This is items 1 and 2 of
[what to build next](../reports/ai-production-trial-t7.md#what-to-build-next-in-order)
after the T7 trial. Item 1, performance variation, covers F60, F61 and F62.
Item 2, writing a line, covers F53, F52 and F58. Part 1 also fixes F39 from
T6.

## Why

In T7 the saxophone of "After the Rain" went through nine listening rounds.
The last two that it won made it less regular:

- After version 3 the owner said: "Maybe it's because it's too uniform. A wind
  instrument should have something irregular, but the way it gets louder and
  softer..." The score set every eighth to 0.46 on the beat and 0.70 off it,
  nine velocities in the whole part, and the instrument gave every note the
  same 250 ms sag.
- Round 7 drew velocities from an arc over each phrase, with chance accents
  and ghosts, and gave each note its own breath. Both won.
- Round 8 let one breath run through each phrase instead of a new one on each
  note. It won: "Yes, less artificial."

None of this could be written in MaaC by hand. A Python script drew every
value and wrote it out:

- **457 named curves**, one breath for each saxophone note, a quarter of the
  song's 1745 lines (F61). A note's score-clock curve starts at its own onset
  (§8.1), so notes cannot share one breath across a phrase.
- **Three copied patterns**, 124 notes, so that the tune's repeats would not
  play the same (F62). A placed pattern repeats exactly.
- **Nothing in the instrument varies.** `synth.noise/1` restarts from its seed
  in every voice, so the tongue's "t" is the same burst on all 457 notes, and
  no node gives a note a value of its own (F60).

Writing the line was the other cost. Each note needs an absolute `at`, so the
script computed all 2069 positions (F53), and its own parsing bug put a
120-cent scoop on 383 notes in version 1. The 18 ms lay-back is an
`onset_offset` and a `release_offset` on every saxophone note (F58). Every
note starts its own voice, so a saxophone cannot slur (F52); connected notes
are played only by `synth.bow/1`.

The goal: an AI or a person writes the T7 saxophone directly in MaaC, without
a script, and it sounds at least as good as version 4.

## The existing position, and why this proposal revisits it

[§9](../../MaaC-1-Specification.md#9-finite-patterns-and-composition) says:
"Probability, Euclidean rhythms, reversal, and quantization may be authoring
operations; their materialized events or explicit finite uses are stored." The
[authoring density](authoring-density-proposal.md) and
[strum](strum-voicing-proposal.md) proposals kept humanizing in tools on that
basis. The authoring density proposal brought swing into the language for three
reasons. T7 shows that they apply to variation too:

1. **The long form stays in the source.** The script wrote 457 velocities, 457
   breaths and three copies of the tune. An author reads and edits all of it,
   and cannot tell the accent that was meant from the scatter on top of it.
2. **The intent is lost.** The source does not say how much the part varies or
   where its phrases are. To vary less, or to play a repeat differently, the
   author runs the script again, and a hand edit to its output is lost.
3. **It is an exact map, not a generator.** The variation proposed here is a
   fixed function of a seed and an event's address (§10). It adds and removes
   no events, has no state and no control flow, and the compiler lowers it to
   ordinary velocities, offsets and expression points. `synth.bow/1` already
   draws each player's timing this way, from its seed and the line's address.

Phrases (part 1A) need no change to §9: a phrase is a curve written by the
author. Variation (part 1B) changes §9's sentence. That is the owner's
[decision 1](#decisions).

## The proposal at each boundary

| Part | Boundary | What changes |
| --- | --- | --- |
| 1A. Phrases | Language | A pattern child that shapes the dynamics and expression of the notes under it. Lowered by the compiler; the plan does not change |
| 1B. Variation | Language | A `vary` declaration that a placement names, as it names a `groove`. Lowered by the compiler; the plan does not change |
| 1C. Per-note values in instruments | Engine | A voice-only node, `synth.random/1`, and a per-note seed for `synth.noise/1` |
| 2A. Notes in succession | Language | `after` on a note or chord, instead of `at` |
| 2B. Connected notes for any instrument | Language and engine | An instrument opts in; envelopes say whether a tongued note restarts them; a generic slide |
| 2C. Lay-back | Language | A groove's `delay` |

## 1A. Phrases

A `phrase` is a pattern child. It has `at` and `length` in pattern-local q,
like a note's `at` and `dur`, and up to three curves:

```maac
curve arc { clock = score; points = [(0q, 0.92, linear), (5q, 1.08, linear), (8q, 0.95, step)]; }
curve rise_fall { clock = score; points = [(0q, 0.12, linear), (5q, 0.4, linear), (8q, 0.2, step)]; }

pattern head_a {
  length = 32q;
  note s1 { at = 1/2q; dur = 1/2q; pitch = D3; velocity = 0.6; }
  // ...
  phrase p1 { at = 1/2q; length = 8q; dynamics = &arc; pressure = &rise_fall; }
}
```

- **Which events.** A phrase covers every note, chord member and hit of its
  pattern, including those of its `use` children after expansion, whose onset
  lies in `[at, at + length)`. Positions are pattern-local and before the
  groove, so the author reads them from the score.
- **`dynamics`** is a score-clock curve of nonnegative multipliers. A covered
  event's velocity is multiplied by the curve's value at its onset, measured
  from the phrase's `at`, and clamped to 1. This is a crescendo or a
  diminuendo, or an arc over a phrase.
- **`pressure` and `timbre`** are score-clock curves of values in `[0,1]`. A
  covered note that has no expression of that kind receives one: the phrase's
  curve from the note's onset to its gate end, rebased to the note's onset.
  Notes of one phrase therefore read one continuous breath. A note's own
  expression of the kind replaces the phrase's (decision 3), and an instrument
  without that expression is `E_CAPABILITY`, as for a note's own.
- **Nesting.** Phrases may overlap and nest. Dynamics multiply, as transpositions
  add. For `pressure` and `timbre`, the phrase nearest the note wins: the one in
  the deepest pattern, then the one that starts later, then the one whose ID
  sorts last.
- **Stretch and repetition.** A phrase repeats and stretches with its pattern;
  a stretch multiplies its positions and its curves' positions, as it does a
  score-clock expression's.
- **Lowering.** The compiler writes each covered note's final velocity and its
  expression as score-clock points. The plan holds them as it holds any note's,
  so the renderer and plan format do not change. The points count towards the
  published expansion limits (§23).

Phrases replace T7's 457 breath curves with the 51 phrases of the part, which
can share a few curves. A crescendo across the repetitions of a roll (F39) is a
phrase in a pattern that uses the roll.

## 1B. Variation

A `vary` declares how much a part departs from what is written. A placement
names it with `vary = &id`, as it names a groove.

```maac
vary loose { seed = 7; velocity = 0.06; timing = 8ms; }
place sax_head { pattern = &head_a; track = &sax_notes; at = 16q; count = 2; groove = &swing; vary = &loose; }
```

- `seed` is a nonzero unsigned 32-bit integer. `velocity` is a dimensionless
  amount from 0 to 1/2 and `timing` a duration from 0 to 50 ms; both default to
  0.
- **The draw.** For each note, chord member and hit, the compiler takes
  numbers in `[0,1)` from a fixed hash of the seed, the event's §10 address
  and the field's name, through the same documented generator as
  `synth.bow/1`. A field's offset is `amount × (u1 + u2 − 1)`, a triangular
  spread within ±`amount`, most often near 0.
- **Velocity** gains its offset after phrases and is clamped to `[0,1]`.
- **Timing** moves the onset and the gate end together, by the same physical
  offset, so a note keeps its length. A gate that ends where another note on
  the track starts, as written, moves with that note's onset instead, so
  notes that touch still touch and connected lines stay connected.
- **Order.** Variation applies after expansion, stretch, transposition, groove
  and phrases, and before overrides. An override's value is final and is not
  varied.
- **Identity.** A draw depends only on the seed and the address. Adding a note
  changes no other note's variation, a moved note keeps its own, and each
  repetition of a placed pattern has its own address, so repeats differ
  without copies (F62). A render range gives the same values as a full render.
- **Lowering.** The compiler writes final velocities and offsets. The plan does
  not change, and `maac check --json` can list a placement's final values so
  that a tool or person can inspect them.

Variation never adds, removes, reorders or retunes an event.

## 1C. Per-note values in instruments

- **`synth.random/1`** is a voice-only source with no inputs and one optional
  configuration, `seed` (nonzero, 32 bits). It emits a constant in `[−1,1]`,
  drawn from the seed and the address of the voice's note. On a joined note
  (§8.3) it moves to the joined note's value over 100 ms, as the velocity
  does. An instrument maps it like any source: a few cents of tuning, a little
  brightness, a vibrato that is not the same on every note.
- **`synth.noise/1`** gains `config.per_note`, default false. When true, each
  voice starts its sequence from the seed mixed with its note's address, so
  each tongued "t" is a different burst. Without it nothing changes.

These are engine changes with new behaviour only where an instrument opts in.
Existing libraries keep their sound; a later version may use them.

## 2A. Notes in succession

A note or chord may give `after` instead of `at`: a reference to another note or
chord of the same pattern, and an optional `rest`, a nonnegative q duration,
default 0. It starts where that one ends, after the rest.

```maac
note s1 { at = 1/2q; dur = 1/2q; pitch = D3; velocity = 0.6; }
note s2 { after = &s1; dur = 1/2q; pitch = F3; velocity = 0.66; }
note s3 { after = &s2; dur = 3/2q; pitch = A3; velocity = 0.7; }
note s4 { after = &s3; rest = 1/2q; dur = 1/2q; pitch = G3; velocity = 0.6; }
```

- `at = after.at + after.dur + rest`, computed exactly in pattern-local q
  before any transformation. Giving both `at` and `after`, or neither, is
  `E_RANGE`. A reference outside the pattern, or to a hit or a message, is
  `E_REFERENCE`, and a chain that returns to itself is `E_PATTERN_CYCLE`.
  The result must still satisfy `0 <= at < length`.
- Identity does not change: each note keeps its own ID, address and fields,
  and the syntax tree stays keyed by ID, with no order among children
  (§20.1). Lengthening a note moves the notes after it; that is the point.
- Editing a note's position means setting `at`, or `after` and `rest`. Deleting
  a note that another follows is `E_REFERENCE` until the follower is
  re-pointed.

## 2B. Connected notes for any instrument

[§8.3](../../MaaC-1-Specification.md#83-connected-notes) already defines
joining for any receiver that plays connected notes, and the renderer's voice
already continues a note generically. Only `synth.bow/1` opts in. This part lets
any instrument opt in and says what `slur` and `slide` do there.

- **Opting in.** An instrument's voice gives `connected = true`. Notes that
  touch on its track then join, by §8.3's rules, as on a bowed instrument.
  Without it, every note starts its own voice, as now.
- **Envelopes.** `synth.adsr/1` gains `config.on_join`, `hold` (default) or
  `restart`. On a joined note without `slur`, a re-articulation, each
  `restart` envelope enters its attack again from its present value, without
  dropping to zero; `hold` envelopes continue. On a slurred note, nothing
  restarts. A saxophone holds its amplitude and restarts its tongue and its
  dark start, so a tongued note is articulated without a gap and a slurred one
  is not articulated.
- **Slide.** A joined note with `slide` moves the voice's base pitch from the
  earlier note's, in cents, linearly over `slide` from its onset. Without it
  the pitch changes at the onset. The note's own pitch expression, such as a
  scoop, adds to it as before.
- **No reading ahead.** A generic voice reads nothing before a note starts, so
  the 500 ms look-ahead of §8.3 is not used and a live adapter needs no added
  latency for it.

## 2C. Lay-back

A groove gains `delay`, a duration from −50 ms to 50 ms, default 0s. It is
added to every onset and every gate end of the placement's events, after the
swing map, as if each `onset_offset` and `release_offset` were larger by that
much. A negative delay pushes ahead of the beat. The saxophone gets its own
groove, the band keeps theirs:

```maac
groove swing { grid = 1/2q; ratio = 3/5; }
groove sax_feel { grid = 1/2q; ratio = 3/5; delay = 18ms; }
```

## Effect on "After the Rain"

With every part, the saxophone could be written without a script:

- its 333 notes, without the three copies, written in succession with `after`,
  from one `at` per phrase;
- 51 phrases with dynamics and breath from about ten shared curves, instead of
  457 curves;
- one `vary` on the saxophone's placements, so that each repeat of the tune
  differs, instead of three copied patterns;
- lay-back in `sax_feel`, instead of 914 offsets;
- `connected = true`, with `slur` where a player slurs, and `per_note` noise
  for the tongue.

The saxophone's part would shrink from about 960 lines to about 400, every one
written by hand.

## Plan

1. **Phrases (1A)** in the compiler, spec text in §9, conformance vectors.
2. **Succession (2A)** and **lay-back (2C)**, both small compiler changes.
3. **Variation (1B)**, if the owner takes decision 1: the draw, the lowering,
   a listing in `maac check --json`, and §9's new text.
4. **Per-note values (1C):** `synth.random/1` and `per_note` noise.
5. **Connected notes for any instrument (2B):** the opt-in, `on_join` and the
   generic slide in the voice, and the spec text in §8.3.
6. **"After the Rain" version 5:** the saxophone rewritten by hand with parts
   1A to 2C, and listening rounds (decision 8).
7. **Documentation:** the specification, the instruments guide, the producer
   guide, the capability matrix and the CHANGELOG.

## Acceptance

- **By hand.** The AI writes the saxophone of version 5 directly in MaaC, with
  no script. No breath curve belongs to a single note, and no pattern is a
  copy of another.
- **No loss.** Blind, version 5 without slurs is heard as the same as, or
  better than, version 4.
- **Slurs.** A blind pair of version 5 with and without slurs and the per-note
  tongue tells the owner whether they help. They are kept only if they do.
- **Exact.** Each phrase, `after` chain, `vary` and `delay` lowers to the
  same plan as writing the values out by hand; tests compare the two.
- **Stable variation.** Repeated compiles give identical plans. Adding a note
  changes no other note's values; changing `count` keeps the surviving
  repetitions'; a render range matches the full render sample for sample.
- **Connected.** Tests cover `hold` and `restart`, slurred and tongued joins,
  slides, and a join whose earlier note was varied in time.
- **Unchanged songs.** Every existing example, fixture and trial song renders
  bit-identically.
- **Diagnostics.** Each new error has a test and an entry in the diagnostics
  table.

## Versioning

Phrases, `vary`, `after`, `rest`, a groove's `delay`, `connected`,
`on_join` and `per_note` make invalid source valid and change the meaning of
no valid source. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) they can join
`maac 1`. Parts 1A, 1B, 2A and 2C lower to existing plan fields. `synth.random/1`
is a new processor identity, and `connected`, `on_join` and `per_note` are new
configuration in the plan's instrument graph, which renderers that do not know
them reject.

## Rejected alternatives

- **Keep variation in tools,** as §9 says today: a `maac` command or a script
  that writes the values into the source. This is the alternative in decision
  1. It keeps the language smaller, but leaves T7's three costs: the long form,
  the lost intent, and copied patterns for repeats.
- **Inline curves (F61).** Allowing `curve = { ... }` inside an expression
  would shorten each of the 457 curves but keep one per note. Phrases remove the
  need T7 found, and a second way to write a curve is not needed yet.
- **Adding a note's expression to its phrase's.** A sum of curves on different
  clocks, a note's seconds-clock scoop and a phrase's score-clock breath, is not
  exact in q. The note's own replaces the phrase's instead; a phrase's curve can
  itself swell over a held note, since it is in score time.
- **A `line` leaf with a list of pitches and lengths,** the alternative in
  decision 4. It is shorter, but its notes are numbered by position, so
  inserting one renumbers the rest and their overrides, and each note's fields
  and expressions would need a new form inside the list.
- **"No `at` means after the note above."** The syntax tree keys children by ID
  without order (§20.1), so the order in the file cannot carry meaning.
- **Lay-back on the track,** the alternative in decision 5. A track groups
  events; it has no timing today, and a groove already is a part's feel.
- **A woodwind model for slurs.** T7's reed waveguide lost ("just like a
  vuvuzela"); generic joining works with the saxophone that won.

## Not proposed

- **Generators:** probability of notes, Euclidean rhythms, random pitches.
  Variation changes values only.
- **Accents in a groove,** a velocity per grid step. Velocities stay written,
  with phrases and variation on top.
- **Varying a phrase's curves** from repeat to repeat. Variation applies to
  each note.
- **Changes to `std/studio`, `std/orchestra` and the other libraries.** A later
  version may use `synth.random/1` and connected notes.

## Cost and risk

- **§9's boundary moves** if the owner takes decision 1. The defence is that
  the map is exact, keyed by address and lowered at compile time; the risk is
  that later requests for real generators cite it. The proposal draws the
  line at values: no event is added or removed.
- **Connected notes reach every instrument.** The voice, capacity and analysis
  paths that `synth.bow/1` uses become reachable from any graph that opts in.
  Instruments that do not opt in are unaffected.
- **Two ways to place a note.** `at` and `after` coexist; tools that move notes
  need to handle both.
- **Work.** Phrases and variation add no render work; the expression points
  they lower count towards the expansion limits. `synth.random/1` is one value
  per voice.
- **The owner's taste is the test.** Rounds 7 and 8 won with the script's
  values; version 5 draws them differently, so it needs its own blind round.
  T7 also suggested an order bias towards b, so version 5's round sends one
  pair twice with the letters swapped.

## Decisions

The owner accepted every recommendation on 2026-11-02 ("go with the
recommendations").

1. **Variation in the language:** a `vary` that a placement names, its values
   drawn from a seed and each note's address, lowered by the compiler
   (recommended). The alternative keeps §9 as it is and a tool writes the
   values into the source.
2. **Phrases:** a `phrase` that shapes the dynamics, breath and timbre of the
   notes under it (recommended). The alternative is one curve per note, made
   shorter with inline curves.
3. **A note's own curve and its phrase's:** the note's own replaces the
   phrase's (recommended). Adding them would need curves on different clocks
   to be summed.
4. **Writing a line:** `after` on each note, naming the note before it
   (recommended). The alternative is a `line` with a list of pitches and
   lengths: shorter, but its notes are numbered by position.
5. **Lay-back:** on the groove (recommended), or on the track.
6. **Slurs for every instrument:** an instrument opts in, and its envelopes
   say whether a tongued note restarts them (recommended). The alternative
   waits for a woodwind model.
7. **Per-note values in instruments:** `synth.random/1` and a per-note seed for
   noise, now (recommended), or later.
8. **Listening:** two blind rounds on "After the Rain" version 5: without
   slurs against version 4, then with slurs and the per-note tongue against
   without, one pair sent twice with the letters swapped (recommended).

## Results

### Part 1A: phrases (2026-11-02)

[§9.3](../../MaaC-1-Specification.md#93-phrases) specifies phrases as
proposed. Points the proposal left open:

- **Ties.** Two phrases of one pattern that start together and both give a
  breath: the one whose ID sorts last in byte order gives it.
- **Curves.** A phrase's curves use the score clock (`E_UNIT` otherwise) and
  `step` and `linear` segments only (`E_RANGE`), so every cut of a curve at a
  note's onset and gate end is exact.
- **Hits.** Dynamics shape hits too, so a phrase around a kit roll is a
  crescendo; pressure and timbre reach only notes.
- **Editing.** An edit to a phrase reports every event of its pattern, and
  of the patterns it uses, as possibly affected. Materializing an instance
  copies its phrases, with private copies of their curves; the copy keeps
  each pattern's coordinates, so the phrases shape the same events.
- **Cost.** Each phrase looks at every event its pattern expands, and that
  work counts towards the expansion bound.

Tests in `tests/phrase.rs` compare a phrase with the same velocities and
breaths written by hand, note by note, and cover precedence, nesting,
stretch, chord members, inserts, overrides, each error, edit impact and
materialization.

### Parts 2A and 2C: notes in succession and lay-back (2026-11-02)

- **`at = after(id, rest)`, not `after = &id`.** References name top-level
  objects, and library imports rewrite them to their resolved IDs, so a
  sibling cannot be a reference. A constructor in `at` keeps one position
  field per note: `at = after(s1)` or `at = after(s1, 1/2q)`, with the
  note's ID as a bare name. `after` joins the core constructors of
  [§3.2](../../MaaC-1-Specification.md#32-constructors).
- **Errors.** An `id` that names no note or chord of the pattern is
  `E_REFERENCE`, a chain that returns to itself `E_PATTERN_CYCLE`, a
  negative rest `E_RANGE`, a rest in seconds `E_UNIT`, and `after` on a
  hit, use, phrase or inserted leaf `E_RANGE`.
- **Editing.** An event in succession depends on every note it follows, so
  lengthening a note reports the notes after it. A chord without pitches
  that follows another reads the harmony at its resolved position, and
  materializing keeps `after`, since the copy keeps the leaves' IDs.
- **Delay.** A groove's `delay` is added to each moved event's onset offset
  and each moved note's release offset. An override that sets an offset
  replaces the sum; inserts, which a groove does not move, keep their own.
  A negative delay that takes the first note before the score start is
  `E_INTERVAL`, as any offset would be.

`tests/succession.rs` compares lines written with `after` against the same
positions written out, including chords that read a harmony, and covers each
error, edit impact and materialization; `tests/groove.rs` compares a delay with
the same offsets written on every note.

### Part 1B: variation (2026-11-02)

[§9.4](../../MaaC-1-Specification.md#94-variation) specifies `vary` as
proposed, and §9's sentence on probability now names it as declared
variation.

- **The draw** is written out in the specification: FNV-1a of the address
  and the field's name from a basis mixed with the seed, then the first two
  SplitMix64 outputs, the generator `synth.bow/1` already used, now shared.
  The offset is `amount × (k1 + k2 − 2^53) / 2^53` from their top 53 bits,
  an exact rational. `spec/check_spec.py` computes one draw independently,
  and `tests/vary.rs` checks the compiler against its own copy of the
  formula.
- **Touching notes.** A note's gate end moves with the onset of the note it
  touches when exactly one note of the same chord member index starts
  there, as joining requires.
- **Listing.** The proposal offered a listing in `maac check --json`. The plan
  that `maac compile` writes already holds every final velocity and offset,
  so no listing was added.
- **Materializing** a placement with a `vary` is refused (`E_CAPABILITY`):
  the copies have other addresses, so they would draw other values.
- **Editing.** A change to a `vary` is reported as possibly affecting every
  event, as a change to any declaration outside patterns and placements is.

### Part 1C: per-note values in instruments (2026-11-02)

- **`synth.random/1`** emits `2u − 1` for each note, `u` the first unit draw
  of the shared generator from its seed mixed with the note's address. Its
  default seed is 1, as `synth.bow/1`'s is. On a joined note it glides to
  the joined note's value over 100 ms, through the same code as the velocity.
- **`per_note`** on `synth.noise/1` starts each voice from the high 32 bits of
  the seed mixed with the address, or from the seed where they are zero. A
  plan records it only when true, so plans of existing noise are unchanged.
- **Tests** in `tests/per_note_values.rs`: each note's noise differs with
  `per_note` and is identical without it, a value is constant through its
  note and differs between notes, and a render range hears the values of
  the whole render.

### Part 2B: connected notes for any instrument (2026-11-02)

As proposed: `connected = true` on a voice, `config.on_join` on
`synth.adsr/1`, and a generic slide, linear in cents from the joined note's
onset. [Connected notes](../instruments.md#connected-notes) gives the rules.

- **Restart from the present level.** A restarting envelope's attack runs
  from its level at the join to 1, so a held note's level never dips. An
  envelope that never restarts computes exactly as before.
- **Bowed sections are untouched.** They keep their own slurs and slides
  from the line they read; the generic slide and restarts apply only to
  voices without a bowed section.
- **Plans** record `connected` and `on_join` only when they differ from the
  defaults, so existing plans and identities do not change.
  `GraphProcessor::Adsr` gains `on_join`, with `GraphProcessor::ADSR` as the
  holding envelope, and `GraphProgram` gains `connected`.
- **Tests** in `tests/connected_any.rs`: notes join only on a connected
  voice, a tongued join restarts the tongue and a slurred one does not, the
  amplitude holds through a join, a slide is halfway in cents halfway
  through, a varied line stays joined, and a random value glides.

### "After the Rain" version 5 and the listening rounds (2026-11-02)

- **By hand.** The AI wrote the saxophone directly in MaaC: 341 notes in
  eight patterns (not 333, as [estimated](#effect-on-after-the-rain)),
  26 phrases on ten shared curves, the lay-back in `sax_feel` and one
  `vary`. No breath curve belongs to one note and no pattern is a copy. The
  song went from 1745 lines to 1203. The notes of the bass, piano and drums
  are still version 4's.
- **No loss (round 10).** Version 4 against version 5 on version 4's
  instrument, in the head and in the solo, the letters swapped between the
  two. The owner heard b as "strangely smoother" in both, so the version
  made no audible difference, and the order bias T7 suspected is now
  likely.
- **Slurs (round 11).** With the connected voice, slurs into the beat, the
  restarting tongue and per-note tongue noise, against without; the owner
  played each pair both ways round and heard the slurred version as "a bit
  more dynamic" in both pairs. Kept.
- **Delivery.** −14.79 LUFS and −1.10 dBTP, passed, at 11.3 billion units of
  work. [`examples/rain/rain.maac`](../../examples/rain/rain.maac) is
  version 5.

