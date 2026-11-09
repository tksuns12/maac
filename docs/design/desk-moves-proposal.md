# Desk moves with the music: proposal

**Status:** accepted 2026-11-09, when the owner took every recommended
[decision](#decisions).
This is item 1 of
[what to build next](../reports/ai-production-trial-t9.md#what-to-build-next-in-order)
after the T9 trial, and it closes F68.

## Why

A dub is played on the desk: parts drop out and back in, and single hits
are thrown into an echo. In "Low Tide", T9's dub, every one of those moves
is a point in a curve that covers the whole song.
[§12](../../MaaC-1-Specification.md#12-curves-and-parameter-automation)
allows at most one global automation per parameter, so a send has one
curve, and each throw is two step points at a position counted by hand.
Version 3's desk is 11 curves of 99 points.

| A producer wants | What MaaC says today | Question |
| --- | --- | --- |
| Throw one snare hit into the echo | Two points in the send's one curve: bar 14's snare is `(54q, 5/2, step), (109/2q, 0, step)`, where 54q is the placement's 16q plus nine 4q repetitions plus the hit's 2q | Can write, but the intent is lost |
| The same throw again, later | The same curve at 6q and 14q on one send is `E_AUTOMATION_WRITER`. The throw's points are copied into the one curve | Can write, but only by copying |
| Louder throws | Version 3 raised its snare and guitar throws by editing 15 points one by one | Can write, but the intent is lost |
| Move the melodica's second phrase | Its 8 throw points stay where they were | Can write, but the intent is lost |
| Drop the bass out for two bars in each dub | Points in the bass fader's one curve, at positions counted by hand | Can write, but the intent is lost |

By the
[three questions](design-principles.md#evaluate-features-at-the-appropriate-boundary),
the moves change the sound and can be written today, but the written form
loses the intent. A tool could compute the points, but then changing a
throw would mean running the tool again. As with T7's breath curves, which
[phrases](../../MaaC-1-Specification.md#93-phrases) replaced, the intent
belongs in the document.

T9 also tried the other way to throw a hit: a second node that plays copies
of the thrown notes into the echo only. Under `vary` the copy is a
different hit: bar 2's snare copy plays 2.01 ms late at velocity 0.791,
while the dry hit plays 1.37 ms late at 0.807. It also doubles the
instrument's work.

The goal: a desk move is written once, where the music is, and moves with
the music.

## The proposal at each boundary

| Part | Boundary | What changes |
| --- | --- | --- |
| 1. Several moves on one parameter | Language and plan | A parameter may have several automations, as long as they do not overlap in time |
| 2. Moves in a placement | Language | An `automation` inside a `place`, timed from the placement or from one of its events |

## 1. Several moves on one parameter

An automation's span runs from its anchor to its last point. Two
automations on one parameter may not overlap. The later one must start at
or after the end of the earlier one's span, and two automations may not
start at the same instant. An overlap is `E_AUTOMATION_WRITER`.

At any instant, the automation with the latest anchor at or before that
instant applies. After its last point, its final value holds until the next
automation starts. Before the first one starts, the node's base value
applies. This is exactly the curve the author would get by joining the
automations' curves by hand. There is no blending, no priority and no
invented crossfade, as §12 requires today.

```maac
curve out_two_bars { clock = score; points = [(0q, 0, step), (8q, 1, step)]; }
automation bass_out_dub_1 { target = &bass_fader.params.gain; curve = &out_two_bars; at = 64q; }
automation bass_out_dub_2 { target = &bass_fader.params.gain; curve = &out_two_bars; at = 136q; }
```

A document with one automation per parameter means what it means today.
Its plan bytes and execution hash do not change.

## 2. Moves in a placement

An `automation` can be a child of a `place`, as an `override` is. Its `at`
then counts from the placement's own `at`. Alternatively it names `event`,
a relative event address in that placement written as an override's is
([§11](../../MaaC-1-Specification.md#11-instance-overrides-and-inserts)).
Its `at` then counts from that event's final score onset, defaults to 0q,
and may be negative, so that a send can open just before the hit.

```maac
curve snare_throw { clock = score; points = [(0q, 5/2, step), (1/2q, 0, step)]; }
place snare_main {
  pattern = &drop_snare_ghost; track = &snare_notes; at = 16q; count = 49;
  vary = &hands; groove = &lilt;
  automation bar_14 { event = "9/s"; target = &snare_send.params.gain; curve = &snare_throw; }
  automation bar_18 { event = "13/s"; target = &snare_send.params.gain; curve = &snare_throw; }
}
```

- **The move follows the music.** Moving the placement, changing its
  groove, or overriding the event's `at` moves the throw with the event.
  Raising every snare throw changes one value in `snare_throw`.
- **The written position.** The anchor is the event's final score onset,
  after expansion, stretch, phrases, groove and overrides. Physical
  offsets, including `vary`'s timing, do not move it; a negative `at`
  covers them.
- **An event that is not there** is `E_REFERENCE` at the automation's
  `event`. This covers an address that names no event, an event that an
  override deletes, and a repetition that a smaller `count` removes, as for
  a dangling override.
- **One rule for all moves.** Part 1's overlap rule covers every
  automation on a parameter, whether global or in a placement.
- **Lowered.** The compiler gives each automation in a placement its
  position in the score. The plan holds it as a lane, as it holds a global
  automation today, so part 2 adds nothing to the plan.

## Effect on "Low Tide"

Version 4 would rewrite only the desk:

- The 27 throws become automations in their placements, each naming the
  hit it throws. Their levels live in three curves, so raising the snare's
  throws is one number, not ten points.
- The drop-outs and the three runaways become one automation each, with
  named curves used more than once.
- With every move at the position that version 3 counted by hand, version
  4 renders sample for sample as version 3.

Every other song is unchanged and renders bit-identically.

## Plan

1. **Several moves (part 1):** the rule, the compiler, plan validation,
   and the engine, which evaluates the lanes on a parameter in time order.
2. **Moves in a placement (part 2):** the child, its anchors, its
   diagnostics, and editing: patch operations, normalization, and
   `materialize-instance`, which must carry a placement's automations to
   the new pattern's addresses.
3. **"Low Tide" version 4** in `examples/dub/dub.maac`.
4. **Documentation:** §10, §11, §12 and the diagnostics table in §23, the
   producer guide, the capability matrix and the CHANGELOG.
5. **Next:** F70, feedback inside effects.

## Acceptance

- **Same samples.** One curve at 6q and 14q on one send renders as the
  single curve an author would join by hand. An automation in a placement
  renders as a global automation at its resolved position. "Low Tide"
  version 4 renders sample for sample as version 3.
- **Moves with the music.** Moving a placement by 4q moves its
  automations by 4q. A groove or override that moves an event moves its
  throw.
- **Refused.** An overlap is `E_AUTOMATION_WRITER` at the later
  automation. An `event` that names nothing is `E_REFERENCE`.
- **Unchanged.** Every existing example, fixture and trial song renders
  bit-identically. Documents with one automation per parameter keep their
  plan bytes and execution hashes.
- **Diagnostics.** Each new error has a test and an entry in the
  diagnostics table.

## Versioning

Part 1 makes invalid source valid, since several automations on one
parameter were `E_AUTOMATION_WRITER`, and changes the meaning of no valid
source. Part 2 adds a child to `place`. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) both can
join `maac 1`. A plan with several lanes on one parameter is new. Renderers
before this change reject it with `E_AUTOMATION_WRITER`, which is the right
failure.

## Rejected alternatives

- **Moves in patterns.** A pattern does not know where it plays: the same
  pattern can play on several tracks, and a move names one node's
  parameter. A move that always goes with a riff goes in the riff's
  placement.
- **A throw as a field of a note or hit.** Which hits are thrown is chosen
  per repetition, not per written hit, and the hit would then name a desk
  node.
- **Blending, priority or last writer wins.** §12 rules these out: they
  make the sound depend on source order or on a crossfade nobody wrote.
- **A curve that repeats every few bars.** It fits regular moves, but dub
  throws are irregular, and it would leave them at absolute positions.
- **Positions relative to regions.** Regions name sections for analysis;
  placements carry the positions. A move in a placement already moves with
  the music.

## Not proposed

- **Following the event's micro-timing.** A move anchored at the hit's
  physical onset would follow `vary`'s timing. It would need a plan anchor
  made of a score position plus seconds, and a negative `at` already
  covers the few milliseconds.
- **One automation for several events,** as `events = ["9/s", "13/s"]`.
  Each move keeps its own ID, so it can be edited and removed alone. This
  can be revisited if the line count becomes a problem.
- **F69, a kit piece's own output,** and **F70, feedback inside effects.**
  They are items 3 and 2 of T9's list.

## Cost and risk

- **One trial asked for this.** Dub is the extreme case. Drop-outs, rides
  and swells in any mix gain from part 1, but throws tied to hits may
  matter mostly to dub and its relatives.
- **Editing carries most of the work.** A placement's automations must
  survive patch operations, normalization and `materialize-instance`, the
  same paths that overrides and inserts take today.
- **The engine reads lanes in time order.** Plan validation sorts the
  lanes on each parameter and checks for overlap. The automation evaluator
  switches lanes at their anchors. Both are small, but they touch every
  render.

## Decisions

The owner accepted every recommendation on 2026-11-09 ("yes").

1. **Several moves on one parameter,** none overlapping (recommended).
   The alternative keeps one curve per parameter and lets a curve repeat.
2. **Where a throw is written:** in its placement, naming the event it
   throws (recommended). The alternative keeps automations global and lets
   their `at` name a placement and an event.
3. **When a throw starts:** at the event's written position, with a
   negative `at` to open early (recommended). The alternative follows the
   event's micro-timing from `vary`.
4. **Listening:** none, since version 4 renders sample for sample as
   version 3 (recommended). Then F70.
