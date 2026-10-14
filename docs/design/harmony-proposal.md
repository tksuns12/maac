# Harmonies: proposal

**Status:** accepted 2026-10-14, when the owner took every recommended
[decision](#decisions), and implemented the same day. The normative contract
is [§8.2](../../MaaC-1-Specification.md#82-chords); [Results](#results)
compares it with this proposal. This is item 2 of
[what to build next](../reports/ai-production-trial-t4.md#what-to-build-next-in-order)
in the T4 trial: F26, a chord progression written once for every part. It
is a language change. It builds on the
[strum and voicing proposal](strum-voicing-proposal.md), whose voicings a
`use` or `place` can already supply.

## Why

"Lantern Street" plays the same progression on four parts: piano, strummed
guitar, bass and pad. A voicing belongs to one instrument's register, so each
part names its own voicing bar by bar:

```maac
pattern pno_verse { length = 32q;
  use b1 { pattern = &pno_pulse; at = 0q; voicing = &pno_d; }
  use b2 { pattern = &pno_pulse; at = 4q; voicing = &pno_a; }
  use b3 { pattern = &pno_pulse; at = 8q; voicing = &pno_bm; }
  // five more bars, then the same eight chords again for the strums
}
```

- **The progression is written 92 times.** 67 `use` lines name a voicing,
  one chord names its own, and 24 bass `use` lines name a chord through a
  transposition. Each section's chords appear once per part that plays them.
- **A chord change is several edits.** Changing the verse's seventh bar
  from G to Em means editing the piano, the strums and the bass, and
  working out the bass's new transposition.
- **The bass follows the chords by arithmetic.** Its patterns are written on
  D and moved to each chord with `transpose`: A is −500 cents, B −300, G
  −700 and E +200. The song has 18 such values, each worked out by hand, and
  nothing in the source says which chord a bar plays.

## The existing position, and why this proposal extends it

The [strum and voicing proposal](strum-voicing-proposal.md) let a `use` or
`place` supply a voicing, so one rhythm serves every chord. It did not name
the chords themselves. A progression is still a list of voicings, and every
part needs its own list, because a piano D and a guitar D are different
pitches. What the parts share is the chord names and when they change: the
harmony. This proposal separates the two, as a lead sheet does. A harmony
says which chord sounds when, and each part says how it voices each chord.

The [design principles](design-principles.md) still hold. A harmony is a
finite list of names and positions, and a part's voicings are a finite map
from names to pitch lists. Nothing is inferred: no chord symbol is parsed,
and no voicing is generated.

## Proposed contract

This is a language change to `maac 1`. Every addition lowers at compile time
to the notes a chord leaf already produces. The plan format, the renderer,
the MIDI adapter and `maac query-events` see ordinary chord members.

### A. Harmonies

A `harmony` is a new top-level kind: a named timeline of chord names.

```maac
harmony verse_chords { length = 32q; chords = [(0q, "D"), (4q, "A"), (8q, "Bm"), (12q, "G"), (16q, "D"), (20q, "A"), (24q, "G"), (28q, "A")]; }
```

- `length` is a positive musical duration.
- `chords` is a nonempty list of `(position, name)` pairs. The first
  position is 0q, positions increase strictly and stay under `length`, and a
  name is a nonempty string.
- The chord at position `p` is the last entry at or before `p`. A harmony
  repeats every `length`, as a pattern does.
- A name is a label, not a parsed chord symbol: `"Bm"`, `"C#m7"` and
  `"G/B"` mean only what a part's voicings make of them.

### B. A part's voicings

A `voicings` is a new top-level kind: a part's voicing for each chord name.

```maac
voicings piano_voicings { chords = [("D", &pno_d), ("A", &pno_a), ("Bm", &pno_bm), ("G", &pno_g), ("Em", &pno_em)]; }
```

- `chords` is a nonempty list of `(name, &voicing)` pairs with distinct
  names.
- Each reference names a `voicing`, or the entry is `E_REFERENCE`.

### C. A harmony supplied by a use or placement

`use` and `place` gain optional `harmony` and `voicings` references, which
come together. A chord with neither `pitches` nor `voicing` takes its
voicing from the nearest enclosing `use` or `place` that names a `voicing`
or a `harmony`. With a harmony, it plays the part's voicing for the chord
sounding at its own position:

```maac
pattern pno_pulse { length = 4q;
  chord l1 { at = 0q; dur = 2q; bottom = 1; velocity = 0.5; }
  chord r1 { at = 0q; dur = 1/2q; top = 3; velocity = 0.52; }
  // ...
}
place piano_verse1 { pattern = &pno_pulse; track = &piano_notes; at = 16q; count = 8; harmony = &verse_chords; voicings = &piano_voicings; }
place strum_verse2 { pattern = &strum_light; track = &strums; at = 80q; count = 8; harmony = &verse_chords; voicings = &guitar_voicings; }
```

**Position.** A chord's position is its onset in the supplying `use` or
`place`'s pattern, counting that use's repetitions: repetition `i` of a
pattern of length `L` adds `i * L`. Nested uses below the supplier place the
chord as §9 says. The position is taken before the supplier's own `stretch`
and before the groove, so a swung eighth stays with its bar's chord. Strum
staggers and other physical offsets do not move it.

**Rules.**

- **Nearest wins.** A nearer `voicing` replaces an outer harmony, and a
  nearer harmony an outer `voicing`, for the chords below it.
- **Explicit pitches win.** A chord's own `pitches` or `voicing` ignores any
  supplied harmony.
- **Both or neither.** A `harmony` without `voicings`, `voicings` without a
  `harmony`, and a `harmony` beside a `voicing` on the same use or placement
  are `E_RANGE`.
- **Missing voicings are errors.** A chord name that the part's voicings do
  not map is `E_REFERENCE` at the chord occurrence. It names the chord, the
  name, the harmony, the voicings, and the use or placement that supplied
  them.
- **Everything else as for supplied voicings.** `top` and `bottom` are
  checked at each occurrence, transposition applies to the voiced pitches,
  and member addresses keep their form.

**The bass.** A bass needs no new concept. Its voicings are each chord's
root and octave, and its rhythm picks them with `bottom = 1` and `top = 1`:

```maac
voicing bs_d { pitches = [D2, D3]; }
voicing bs_a { pitches = [A1, A2]; }
voicings bass_voicings { chords = [("D", &bs_d), ("A", &bs_a), ("Bm", &bs_bm), ("G", &bs_g), ("Em", &bs_em)]; }
pattern bs_verse { length = 4q;
  chord r1 { at = 0q; dur = 3/2q; bottom = 1; velocity = 0.72; }
  chord o1 { at = 7/2q; dur = 1/2q; top = 1; velocity = 0.48; }
}
```

The register stays the author's: the bass's A is A1, below its D2, as T4's
transpositions made it.

## Effect on T4

"Lantern Street" rewritten with A, B and C:

- **Harmony.** Four harmonies hold the verse, chorus, bridge and outro. The
  intro plays the verse's first four bars.
- **Parts.** Each part has one `voicings`. Each placement names its rhythm,
  its harmony and its voicings.
- **Rhythm patterns.** Rhythms that change within a section become short
  patterns: the bridge's three held bars and a pulse, and the outro's ending.

The counts cover the harmony and the four chord parts, without comments and
instrument definitions:

| | Non-blank lines | Field values | Characters | Chords named | Bass transpositions |
| --- | --- | --- | --- | --- | --- |
| Today | 226 | 620 | 12,231 | 92 | 18 |
| With A, B and C | 132 | 534 | 9,315 | 23, plus 20 voicing entries | 0 |

- **One edit per chord change.** The verse's seventh bar becomes Em with one
  edit to `verse_chords`, instead of three, and no cents.
- **The source says what it plays.** Each bar's chord is named once, in the
  harmony. The parts name their registers once, in their voicings.
- **Field values fall less than lines.** Each placement now names a
  harmony and voicings, and most a count: 74 values in all. The 91 `use` lines
  of the progression patterns go, apart from 8 in the new rhythm patterns.
- **Same notes.** The rewrite plays exactly the notes of the current song:
  the same pitches, onsets, gates and velocities on every part. Only event
  addresses change, from `piano_verse1/0/b2/0/r1.0` to
  `piano_verse1/1/r1.0`, because the bars are now repetitions of one
  placement. A bass note becomes a one-member chord: `bass_verse1/0/b1/0/r1`
  becomes `bass_verse1/0/r1.0`. The whole file goes from 576 to 484 non-blank lines.

## Rejected alternatives

- **Keep progressions per part.** This is today's position. It keeps 92
  chord namings and 18 hand-worked transpositions in T4's source.
- **One chord track for the whole song**, placed once like a DAW's, which
  every part follows by absolute time. T4's last chorus is the chorus a whole
  step up. A global track would spell it in E, so every part would need
  voicings for E, A, B and C#m as well. Or the track would carry its own
  transposition, which the melody's placement repeats separately. A harmony
  per placement inherits the placement's transposition like everything else.
- **Parsed chord symbols**, where `"Bm7"` yields a root and a quality, and
  the compiler builds voicings or bass notes from them. Voicing is a choice:
  the register, the inversion and the voice leading are the arranger's. The
  principles keep that choice in the source.
- **A `root` pitch for the bass**, such as `pitch = root`. Each harmony
  chord would then need a root pitch and an octave rule. Root-and-octave
  voicings do the same with `bottom` and `top`, and keep the register
  explicit.
- **Chord names as identifiers**, such as `bm`. Chord symbols use `#` and
  `/`, which identifiers cannot hold. Strings keep `"C#m7"` and `"G/B"`
  readable.
- **Looking up the chord after the groove or by score time.** A swung
  eighth near a bar line would change chords. Score time would also break
  when the same placement is reused at another position.

## Not proposed

- **Pushed chords.** A chord struck an eighth before the bar line plays the
  old chord. Write the harmony change at the push, or name the voicing on
  that chord.
- **Picking a middle member**, such as a walking bass's fifth. `top` and
  `bottom` pick only the outer strings. This is the picking-by-string
  proposal that the strum proposal deferred.
- **Melody notes relative to the chord**, such as scale degrees.
- **Harmonies and voicings in libraries.** These wait until songs use them.

## Versioning

The two kinds and the `harmony` and `voicings` fields on `use` and `place`
make invalid source valid. They change the meaning of no valid source. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) they can
join `maac 1`. The plan format does not change, because chords lower to the
same note events.

## Acceptance

**Fixtures**

- A harmony supplied by a `place` with `count` gives each repetition the
  chord at its position. A pattern longer than the harmony wraps around it.
  `stretch` and a groove do not move the lookup.
- Nested suppliers resolve as stated: a `voicing` inside a harmony, a
  harmony inside a `voicing`, and a chord's own pitches.
- Rejected with the stated codes:
  - a harmony whose first position is not 0q, or whose positions do not
    increase, or reach `length`;
  - an empty name, and duplicate names in a `voicings`;
  - a `voicings` entry that is not a voicing;
  - `harmony` without `voicings`, `voicings` without `harmony`, and
    `harmony` beside `voicing`;
  - a chord name the voicings do not map, with every name in the
    diagnostic.
- Transposition applies, `top` and `bottom` are checked at each occurrence,
  and an override that would move to another member is
  `E_INSTANCE_TARGET`.

**T4**

- The rewrite compiles. Every part's notes match the current song's in
  pitch, onset, gate and velocity, and the counts match the table.
- It renders byte-identically, or the results record why not.

**Specification and documentation**

- §4 lists `harmony` and `voicings`. §8.2 adds supply by harmony. §9 and §10
  give `use` and `place` their `harmony` and `voicings` fields.
- The editing protocol, canonical form and conformance vectors cover the new
  kinds and fields. A harmony or voicings edit that changes how many members
  a chord plays is refused when an override would move, as for a voicing
  edit. Materializing a placement keeps the harmony it supplies.
- The [tutorial](../tutorial.md) and the
  [producer guide](../ai-producer-guide.md) show a harmony shared by two
  parts and a bass. The guide's advice to transpose a bass pattern per bar is
  replaced. The T4 F26 row and the [language plan](language-specification-plan.md)
  are updated.

## Cost and risk

- **Cost: medium.** Supply by harmony extends the supplied voicing. The
  lookup needs each chord's position in the supplier's pattern, which must
  travel through expansion as the repetition index does. The new kinds need
  the parser, the normalized form, the editing protocol and canonical
  hashing, as `voicing` did.
- **Risk: a chord's pitches are further from its line.** A chord leaf now
  depends on its placement's harmony and voicings. Diagnostics name all of
  them, and `maac query-events` shows the pitches each chord plays.
- **Risk: a name typed two ways.** `"Bm"` and `"bm"` are different names.
  An unmapped name fails to compile, and the diagnostic lists the names the
  part maps.
- **Risk: a held chord across a change.** A chord struck before a chord
  change keeps its pitches until it ends; nothing retriggers. This is how
  the song's notes already behave.

## Decisions

The owner accepted every recommendation on 2026-10-14.

1. **Harmonies and part voicings (A, B, C):** a `harmony` timeline of chord
   names and a `voicings` map per part, supplied together by a `use` or
   `place`. *Recommended* over progressions per part, over one chord track
   for the song, and over parsed chord symbols.
2. **Chord names:** strings, compared exactly. *Recommended* over
   identifiers, which cannot spell `C#m7` or `G/B`.
3. **Lookup position:** the chord's onset in the supplier's pattern,
   counting repetitions, before stretch and groove, with the harmony
   repeating past its length. *Recommended* over score time or the
   grooved onset.
4. **The bass:** root-and-octave voicings picked with `bottom` and `top`.
   *Recommended* over a `root` pitch keyword.
5. **Versioning:** additive `maac 1`, with no plan change. *Recommended*, as
   for strums and vibrato.
6. **"Lantern Street":** switch the song to the rewrite if it renders the
   same notes. *Recommended*, since nothing audible changes.

## Results

A, B and C were built and tested with
[`tests/harmony.rs`](../../tests/harmony.rs), and "Lantern Street" now uses
them.

**T4.**

- **Notes.** The rewrite gives the same 2,363 notes and 552 hits. Every
  event has the same onset, gate, velocity and target, and every note but
  162 has a bit-identical frequency.
- **Rounding.** The other 162 are bass notes. The old song reached A1, B1,
  G1 and E2 by transposing D2 in cents, which left their frequencies up to
  3 units in the last place off. The rewrite names those pitches directly.
- **Render.** The master therefore differs in 8 of its 12,364,800 samples,
  each by one least significant bit of 24, about −144 dBFS
  (`35c408c2c16f3310…`, against `6eb754d3ba8359a6…`). Nothing audible
  changes, so the README's audio stays as it is.
- **Counts.** They match the table. Characters rose from 8,890 to 9,315
  when the harmonies and voicings were renamed (see below).

**Names.** The first rewrite named its harmonies `verse`, `chorus`, `bridge`
and `outro`, and its voicings after each part. `bridge` and `outro` clashed
with the song's regions, and `piano` with its node, because all top-level
objects share one ID namespace (T3's F23). The song now uses
`bridge_chords` and `piano_voicings`, and the
[producer guide](../ai-producer-guide.md) says to name them by role.

**Differences from the proposal.**

- An edit to a harmony or a part's voicings reports every chord that plays
  from it as affected, not only those whose pitches change. A voicing edit
  still reports only the chords that play the voicing.
- Materializing refuses two cases with `E_CAPABILITY` rather than change the
  chords: a use that supplies a harmony and repeats, whose copies would each
  start the harmony again, and a stretched placement that supplies one, whose
  copies would take over the stretch the lookup ignores. A placement with
  `stretch = 1` keeps its harmony, and its copies play the same chords.

