# Vibrato: proposal

**Status:** accepted 2026-10-14, when the owner took every recommended
[decision](#decisions) and asked for a choice of waveforms: `shape` may
also name a curve that draws one cycle. Both parts are implemented: the
normative contract is [§8.1](../../MaaC-1-Specification.md#81-per-note-expression)
and profile [`default/8`](../analyze.md#profile-maacanalyzedefault8-2026-10-14),
and [Results](#results) compares them with this proposal. This is item 1 of
[what to build next](../reports/ai-production-trial-t4.md#what-to-build-next-in-order)
in the T4 trial: F28, guitar vibrato and bends written in a few values, and
F29, the pitch check for notes with a pitch expression. Part A is a language
change; part B changes `maac analyze`. It follows the
[strum and voicing proposal](strum-voicing-proposal.md), which made the same
kind of case for strums.

## Why

In T4 the owner heard the electric-guitar melody as "more like a synthesizer
than an electric guitar". The instrument's `vibrato` control is one LFO at a
fixed depth, on every note from note-on. The fix that the owner approved
("good") wrote a guitarist's vibrato and bends per note, as pitch
expressions on 18 of the melody's notes:

- **A vibrato is spelled out.** It starts 300 ms after the pick, only rises
  from the fretted pitch and comes back, as a bent string does, and deepens
  over its first cycles. As a curve that is 28 points, which the AI computed
  with a script:

  ```maac
  curve vibrato {
    clock = seconds;
    points = [(0ms, 0ct, linear), (300ms, 0ct, linear), (390ms, 18ct, linear), (480ms, 0ct, linear), (570ms, 28ct, linear), /* 23 more */ (2640ms, 0ct, step)];
  }
  ```

  Nothing in the source says "5.5 Hz, 38 cents, after 300 ms". A different
  rate means recomputing every position, and a different depth every
  value.
- **A bend with a vibrato is a third curve.** A note takes at most one pitch
  expression ([§8.1](../../MaaC-1-Specification.md#81-per-note-expression)),
  so the bent notes that also hold use `bend_vibrato`: the bend's 3 points
  followed by the vibrato again, with every position moved by 120 ms. The
  three curves take 12 lines, 59 points and 1,508 characters.
- **The analysis does not check them.** `pitch_mismatch` skips a note with a
  pitch expression. In T4 that left 41 of the melody's 178 notes unchecked,
  the bent and held ones that are the most exposed (F29).

## The existing position, and why this proposal revisits it

The [design principles](design-principles.md) keep general computation in
tools: "Python or another language can generate MaaC source". Curves already
carry any pitch shape, and a library can export a finished vibrato curve. The
strum proposal answered the same position for strums, and the three reasons
apply here:

1. **The long form stays in the source.** A tool that writes the vibrato
   writes 28 points back, and the author reads and edits them all.
2. **The intent is lost.** The points do not say the rate, the depth or the
   delay. An edit to one point breaks one cycle, and nothing shows it.
3. **This is not a generator.** A vibrato is a fixed, exact shape: a rate, a
   depth, a delay and a rise. It needs no randomness, control flow or
   runtime, and it expands to a finite list of points.

Other ways to get there, and why this proposal does not take them, are under
[rejected alternatives](#rejected-alternatives).

## Proposed contract

### A. Vibrato

A `vibrato` is a new top-level kind: a named, reusable vibrato that a pitch
expression adds to its note.

```maac
vibrato guitar { rate = 11/2Hz; depth = 38ct; delay = 300ms; rise = 450ms; shape = up; }
curve bend_up {
  clock = seconds;
  points = [(0ms, -200ct, step), (40ms, -200ct, linear), (170ms, 0ct, step)];
}

pattern melody { length = 8q;
  note held { at = 0q; dur = 2q; pitch = A4; velocity = 0.6; expression vib { kind = pitch; vibrato = &guitar; } }
  note peak { at = 4q; dur = 2q; pitch = E5; velocity = 0.7; expression bend { kind = pitch; curve = &bend_up; vibrato = &guitar; } }
}
```

**Fields.**

- `rate` is a positive frequency in Hz: cycles per second.
- `depth` is a positive pitch distance in cents that scales the cycle. For
  the built-in shapes it is the full swing, from the lowest point of a cycle
  to the highest.
- `delay` is a nonnegative physical duration from the note's onset to the
  vibrato's start. It defaults to 0 s.
- `rise` is a nonnegative physical duration over which the swing grows from
  nothing to `depth`. It defaults to 0 s, full depth at once.
- `shape` is the waveform: `up`, `down`, `centered`, or a reference to a
  curve that draws one cycle. `up` rises from the pitch and comes back, as a
  guitarist's vibrato does; `down` falls from it; `centered` swings half the
  depth either side. It defaults to `centered`.
- A pitch `expression` gains an optional `vibrato = &name`. It takes
  `curve`, `vibrato` or both, and at least one. Any other kind with a
  `vibrato` is `E_RANGE`.

**Cycles.** A cycle is a `normalized` curve with dimensionless values and
`step` or `linear` segments. Its last value must equal its first, so cycles
join without a jump. The built-in shapes are three such cycles, all
triangles:

| Shape | Cycle points `(position, value)`, all `linear` but the last |
| --- | --- |
| `up` | (0, 0), (1/2, 1), (1, 0) |
| `down` | (0, 0), (1/2, −1), (1, 0) |
| `centered` | (0, 0), (1/4, 1/2), (1/2, 0), (3/4, −1/2), (1, 0) |

A song draws any other waveform the same way. A whole-step trill is a square
cycle:

```maac
curve square { clock = normalized; points = [(0, 0, step), (1/2, 1, step), (1, 0, step)]; }
vibrato trill { rate = 4Hz; depth = 200ct; shape = &square; }
```

**Meaning.** The vibrato repeats its cycle, written out as curve points at
compile time. Let `P = 1/rate`, let the cycle's points be `(u_i, w_i, s_i)`
for `i = 0 … m`, and let `c` be the value the expression's curve ends on, or
0 cents without a curve. From `delay` on, cycle `j = 0, 1, 2, …` puts a knot
at each seconds-clock position `t = delay + (j + u_i) * P` for `i < m`. Its
value is `c + g * w_i`, and its shape is `s_i`. Here
`g = depth * min(1, (t − delay) / rise)`, or `depth` when `rise` is 0, so the
rise scales each knot. Knots continue up to and including the first one at or
past the end of the note's gate, which becomes `step`. All positions and
values are exact rationals.

**Combined with a curve.** The expression's curve must use the `seconds`
clock, or the combination is `E_UNIT`. Its last point must be at or before
`delay`, or it is `E_RANGE`: a vibrato starts once the bend has settled, so
the two never overlap and their sum stays exactly a list of points. The
lowered expression is the curve's points, then the vibrato's knots. The
curve's last value holds from its last point to `delay`.

**Unchanged.** The lowered points are an ordinary `pitch_expression` in the
plan. Everything §8.1 and the [pitch guide](../pitch-expression.md) say still
holds:

- the receiver must support pitch expression;
- the expressed frequency must stay valid over the whole reachable curve;
- during the release, the value at the gate's end holds.

The points count towards the 65,536-point limit and the per-frame work
charge, as a written curve's do. A note shorter than `delay` plays the curve
alone. Without a curve, it plays a single 0-cent point.

- **Physical time.** Rate, delay and rise are in seconds, like a seconds-clock
  curve, so a vibrato does not stretch with `stretch` or the tempo. A
  duration override changes the gate and so the number of knots.
- **Per occurrence.** Knots are computed for each note occurrence from its
  own gate, so one `vibrato` serves notes of every length.

### B. Pitch check for expressed notes

`maac analyze` gains profile `maac.analyze.default/8`. A note with a pitch
expression is checked like any other, under the same lone-note rule.

- **Expected pitch.** The check compares the estimate with the written
  pitch, shifted by the mean of the expression over the estimate window. That
  window starts 30 ms after the onset, past the attack, and covers four
  periods.
- **Too fast to check.** If the expression moves more than 50 cents within
  the window, as a fast slide does, the note is not checked.
- **Report.** The note's `cents` is then its distance from the expressed
  pitch, and the 50-cent limit of `pitch_mismatch` is unchanged.

At 30 ms in, T4's bends still sit 200 cents low and its vibratos have not
started. The check therefore confirms the note's pitch and the start of its
curve. It does not confirm the top of a bend. The curve is exact data the
instrument follows, so a wrong root, a detuned oscillator or a wrong
transposition shows at the start as anywhere else.

## Effect on T4

The T4 melody's three curves, rewritten with part A:

| | Non-blank lines | Field values | Characters |
| --- | --- | --- | --- |
| Today | 12 | 180 | 1,508 |
| With A | 5 | 15 | about 200 |

- **One vibrato serves both.** The bend ends at 170 ms, before the vibrato's
  300 ms delay, so `bend_vibrato` goes. Changing the rate or depth is then
  one edit.
- **Not identical.** The rewrite differs from the approved master in two
  ways:
  - its swing grows linearly over 450 ms, where T4's cycles step through 18,
    28, 34 and 38 cents;
  - on the bent notes, its vibrato starts at 300 ms, where T4's started at
    420 ms.

  Its rate, 11/2 Hz, is T4's within 1%. Written with T4's 180 ms period
  (`rate = 50/9Hz`) and no `rise`, a vibrato gives exactly the points of a
  hand-written curve with a constant 38-cent swing.
- **Part B** checks the 41 notes that profile `default/7` skips.

## Rejected alternatives

- **Keep vibrato in tools.** This is today's position. It keeps 59 points in
  T4's source, and the AI wrote its generator for one song.
- **A repeating span in a curve**, such as `loop = 660ms` to repeat the
  points from 660 ms to the end, like a sample's sustain loop. It shortens
  the list to about 8 points. But the rate is still spread over several
  positions and the depth over several values, a deepening swing is still
  written cycle by cycle, and the intent stays unwritten.
- **Delay and shape on the instrument's `vibrato` control.** That is a
  library change, and the owner put library sound outside this project's
  work after T4. A control is also global to its node: every note of the part
  gets the same vibrato, and a short note gets it as well as a held one.
- **A sine wave evaluated by the renderer.** A sine is not an exact rational
  function. It would put a new field in the plan's `pitch_expression`, so the
  plan version, the renderer, retained plans and interchange would all
  change. A cycle curve with more points comes as close to a sine as wanted,
  as exact points. T4's approved vibrato was a triangle.
- **A waveform function written in the document**, such as an expression in
  `t`. The [design principles](design-principles.md) keep functions and
  evaluation out of compositions. A cycle curve is the same waveform as
  data.
- **Two pitch expressions per note**, a bend and a vibrato, summed. §8.1's
  one expression per kind keeps each note's pitch a single curve. Part A
  gets the sum within one expression, with the overlap ruled out.

## Not proposed

- **A vibrato during a bend.** A step in the curve under a running vibrato
  cannot be written exactly as points, so the curve must end first. A slow
  bend that wavers can still be written as one curve.
- **A vibrato that returns to the pitch at note-off.** In the release the
  expression holds its gate-end value (§8.1), so an `up` vibrato can leave
  its release up to `depth` sharp. It needs a change to release semantics.
- **Tremolo and other kinds.** The same structure would serve a gain or
  timbre expression. This waits until a song needs it.
- **Vibratos in libraries, and supplied by a `use` or `place`.** A library
  could export a vibrato, as it exports curves, and a placement could supply
  one to every note of a part, as with voicings. Both wait until songs use
  vibratos.
- **A rate in q**, following the tempo. Players hold a vibrato rate in
  physical time.

## Versioning

The `vibrato` kind and the expression's `vibrato` field make invalid source
valid and change the meaning of no valid source. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) they can
join `maac 1`. The plan format does not change: the plan holds the lowered
points. Part B is a new analysis profile, `default/8`; reports name their
profile.

## Acceptance

**Fixtures (part A)**

- `up`, `down` and `centered` vibratos, with and without `rise`, lower to the
  exact positions and values above. Their last knot is the first at or past
  the gate's end, and it is `step`.
- A vibrato alone, after a seconds-clock curve that ends before `delay`, and
  after one that ends exactly at `delay`, each lowers as stated.
- A square cycle lowers to `step` knots, a trill; a cycle with uneven
  positions lowers at those positions in every cycle.
- Rejected with the stated codes:
  - `vibrato` on a gain, timbre or pressure expression;
  - a score- or normalized-clock curve with a vibrato;
  - a curve point after `delay`;
  - a nonpositive `rate` or `depth`, and a negative `delay` or `rise`;
  - an unknown `shape`, and a `shape` that names a curve whose clock is not
    `normalized`, whose values carry a unit, which has an `exponential`
    segment, or whose last value differs from its first;
  - an expression with neither `curve` nor `vibrato`.
- A duration override changes the knot count. `stretch` and the tempo do
  not change the knot positions.
- Lowered points count towards the 65,536-point limit.
- A vibrato with `rate = 50/9Hz`, `depth = 38ct`, `delay = 300ms`,
  `shape = up` and no `rise` gives the same plan and a byte-identical render
  as the equivalent hand-written curve.

**Fixtures (part B)**

- A bent note on an instrument with a correct root is checked and passes. The
  same note with its root 100 cents off is a `pitch_mismatch`. A fast slide
  is not checked.
- On "Lantern Street" all 178 melody notes are measured, and no
  `pitch_mismatch` appears. On the songs of the
  [gain reduction proposal](gain-reduction-proposal.md), no note without a
  pitch expression changes.

**T4**

- The rewrite above compiles, and its counts match the table.
- [`examples/lantern/lantern.maac`](../../examples/lantern/lantern.maac)
  switches to the rewrite only if the owner prefers it, or cannot tell them
  apart, in a listening comparison; the README's audio then follows.
  Otherwise the song keeps its curves.

**Specification and documentation**

- §4 lists `vibrato`, and §8.1 gives the expression its `vibrato` field.
- The editing protocol, canonical form and conformance vectors cover the new
  kind and field.
- The pitch guide, the [tutorial](../tutorial.md) and the
  [producer guide](../ai-producer-guide.md) show a vibrato and a bend. The
  guide's pitfall row is rewritten around them.
- [analyze.md](../analyze.md) describes profile `default/8`. The T4 F28 and
  F29 rows and the [language plan](language-specification-plan.md) are
  updated.

## Cost and risk

- **Cost: small to medium.** Part A expands each occurrence's expression,
  like a strum expands its chord. The new kind needs the parser, the
  normalized form, the editing protocol and canonical hashing, as `voicing`
  did. Part B changes one rule in the analyzer.
- **Risk: a triangle where a sine was meant.** The built-in shapes are
  triangles. If a listener hears their corners, a cycle curve with more
  points is smoother.
- **Risk: more points.** A held note carries the cycle's knots `rate` times a
  second: about `2 * rate` for `up` and `down`, `4 * rate` for `centered`. A lead of 200 two-second notes with a 5.5 Hz
  vibrato on every note adds about 4,400 points, well under the limit.
- **Risk: one more way to write pitch.** A curve stays the plain form for
  bends and slides; the guide recommends `vibrato` for periodic pitch only.

## Decisions

The owner accepted every recommendation on 2026-10-14 and widened the
waveform: "shouldn't the waveform be selectable? That is more low-level".
Decisions 2 and 3 are as accepted, with that change.

1. **Vibrato (A):** a top-level `vibrato` kind with `rate`, `depth`,
   `delay`, `rise` and `shape`, named by a pitch expression. *Recommended*
   over keeping vibratos in tools, over a repeating span in a curve, and over
   instrument controls.
2. **Waveform:** a cycle written out as exact points at compile time: the
   built-in triangles, or a `normalized` curve that draws one cycle.
   *Recommended* over a sine evaluated by the renderer, which changes the
   plan, and over a waveform function in the document, which the design
   principles rule out.
3. **Shapes:** `up`, `down` and `centered` built in, defaulting to
   `centered`. *Recommended* over `up` only, which serves guitars but not
   voices or strings.
4. **With a curve:** one pitch expression takes a seconds-clock curve and a
   vibrato, and the vibrato starts at or after the curve's last point.
   *Recommended* over a vibrato during a bend, which points cannot write
   exactly.
5. **Pitch check (B):** expressed notes are checked against the expressed
   pitch, in profile `default/8`. *Recommended* over leaving them unchecked.
6. **Versioning:** additive `maac 1`, with no plan change. *Recommended*, as
   for strums.
7. **Order:** A, then B, each landing with its fixtures.

## Results

### Part A

Part A was built and tested with
[`tests/vibrato.rs`](../../tests/vibrato.rs). The compiler writes each
vibrato into its note's pitch curve after scheduling the note, so the
knots follow the scheduled gate, and the production identity and the point
limit both see them.

**T4.** The rewrite above compiles through the std/studio electric guitar.

- **Size.** Its curve block is 5 lines and 15 values, against 12 lines and
  180 values.
- **Events.** It gives the same 2,363 notes and 552 hits. Every event is
  identical except the 41 melody notes' pitch curves.
- **Points.** Those notes carry 458 points, against 948 in T4. T4's curves
  ran past the end of most notes; the vibrato stops at the first knot past
  each gate.

**Exactness.** A vibrato with `rate = 50/9Hz`, `depth = 38ct`,
`delay = 300ms`, `shape = up` and no `rise` gives the same plan events as the
hand-written curve with the same points. The unchanged song still renders
byte-identically to its approved master (SHA-256 `fae5aacccc86f693…`).

**Listening.** On 2026-10-14 the owner compared the two masters on a phone,
as 192 kbit/s AAC at −14.16 LUFS each. P was the approved master and Q the
rewrite (`6eb754d3ba8359a6…`); the order was drawn at random and revealed
after the verdict: "거의 구별이 안 되네" ("I can barely tell them apart").
As the acceptance allows, "Lantern Street" now uses the rewrite, and the
README's audio is its master.

**Differences from the proposal.**

- An expression with neither `curve` nor `vibrato` is `E_RANGE` with the
  message "expression requires field `curve` or `vibrato`". The other kinds
  still require `curve`.
- A cycle curve's errors are reported at the vibrato's `shape`, naming the
  curve: a clock other than `normalized` or values with a unit are
  `E_UNIT`, and an `exponential` segment or an open cycle `E_RANGE`.

### Part B

Part B was built and tested in [`tests/analyze.rs`](../../tests/analyze.rs):
a bent part in tune passes, the same part a semitone sharp is a
`pitch_mismatch` on all three notes, and a six-semitone slide over 60 ms is
not checked.

- **"Lantern Street".** All 178 melody notes now get a pitch estimate. 149
  are pitched, against 115 under `default/7`, and 29 unpitched, against 22.
  The largest deviation is 1.0 cent, and no `pitch_mismatch` appears. The
  rewrite with part A measures the same.
- **Other songs.** A note without a pitch expression is estimated exactly as
  before, so songs without one report the same notes.

**Differences from the proposal.** An earlier note's release is now
cancelled at the pitch it rings at, its expression's value at the end of its
gate, instead of its written pitch.

