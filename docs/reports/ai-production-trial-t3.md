# T3: a second AI production, with the perception checks

**Date:** 2026-10-11. **Producer:** Claude, an AI model, working in this
repository. **Listener:** the project owner.
**Result:** [`examples/ferry/ferry.maac`](../../examples/ferry/ferry.maac),
"Morning Ferry".

The [T2 trial](ai-production-trial.md) produced "Late Window" and ranked
what to build next. Most of that list has since landed: std/studio, a
limiter, chord leaves, grooves, kits, analysis windows, a built-in delivery
schema and the perception checks. T3 repeated the exercise on a new song to
test those additions and find the next limits. Friction numbers continue
from T2.

## Brief and rules

- **Brief:** an original song with a sung-style melody over a pop
  progression. The AI chose a 112 bpm folk-pop brief; after the first
  version the owner asked for a rock ballad.
- **Tools:** MaaC source written directly, with no generator script, and only
  `maac check`, `maac analyze` (profile `default/4`), `maac query-events` and
  `maac deliver`.
- **Judgment:** the owner listened to each version on a phone and gave the
  verdicts.

## Outcome

- **Form:** 40 bars at 72 bpm, about 133 s plus a 4 s tail. Intro, two verses,
  two choruses and an outro are marked as regions. The verse runs G–D–Em–C
  and the chorus C–G–D–Em, then C–G–D–D.
- **Parts:**
  - picked steel-guitar arpeggios in the verses;
  - overdriven power chords in the choruses;
  - the melody on a driven electric guitar with vibrato;
  - studio bass, kit and pad;
  - a separate snare into a hall reverb.
- **Electric guitar:** no library has one, so the song defines it inline. Each
  voice is a `synth.pluck/1` string with an LFO vibrato. The summed voices go
  through one amp stage: a low cut, a biased `synth.drive/1`, a DC cut, a
  resonant cabinet low-pass and a fizz filter. Automation on the lead's `drive`
  and `level` pushes it harder in the choruses.
- **Mix and master:** a glue compressor at ratio 1.5, then `fx.limiter/1`.
  The chorus is about 2.2 LU louder than the verse in short-term loudness.
- **Delivery:** `maac deliver` passed its checks at −14.56 LUFS integrated and
  −1.10 dBTP true peak.
- **Analysis:** no warnings. No note is silent, buried or mistuned; only
  informational `band_dominance` findings remain.
- **Owner's verdicts:**
  - On the folk-pop version: "fine", but the instruments "still sound a bit
    like old game music", and the song should be a rock ballad.
  - On the rock ballad: "Good. It's good."

## What happened

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote 112 bpm folk-pop: strummed steel guitar, a nylon-guitar melody, std/studio bass, kit, pad and electric-piano stabs. Chord shapes reused through `use` with `transpose` | `maac check` passed on the first try |
| Iterations 1–3 | `analyze --section regions` | The melody sat 8 dB under the strums, the master read −11.9 LUFS, the intro leaned left, and the chorus did not lift because the glue and limiter flattened it. Fixes: rebalance; a lower limiter gain; narrower pans; automation on the strum level; a gentler glue |
| Delivery 1 | `maac deliver` | 88.7 s, −14.43 LUFS, checks passed |
| Verdict 1 | Owner: "fine"; the instruments "still sound a bit like old game music"; the song should be a rock ballad | — |
| Draft 2 | Rewrote it at 72 bpm with the same melody and progression. Built an overdriven electric guitar inline, wrote power-chord choruses, picked verses, toms, crashes and a reverb snare | One error: a node named `acoustic` clashed with the `acoustic` import (F23) |
| Iterations 4–5 | `analyze --section regions` | The lead read 11 dB under the picking in the verses, because a lower amp drive also lowers the amp's output (F22). No finding reported it (F21). An automation on the lead's `level` compensates |
| Delivery 2 | `maac deliver` | 137.3 s, −14.56 LUFS, −1.10 dBTP, checks passed |
| Verdict 2 | Owner: "Good. It's good." | — |

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- `maac check` accepted both hand-written drafts, apart from one clear
  duplicate-ID error.
- Chord leaves, `use` with `transpose` and `count`, and one kit node kept a
  40-bar arrangement to about 500 lines.
- A custom instrument with voice and shared graphs and named controls gave
  the song an electric guitar that no library has.
- Automation on instrument controls made the drive and level changes per
  section one curve each.
- The `default/4` checks raised no false alarms on either version.
- Delivery with the built-in schema passed on the first run each time.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F17 | The libraries have no electric guitar or piano, and the owner still hears the library timbres as "old game music". The AI had to design an amp chain to get a rock sound | Fixed in part: `std/studio/1.1.0` adds an `electric_guitar` and a `piano`, each approved in listening rounds. On the song switched to the library guitar the owner said the Game Boy quality is "definitely mostly gone". The piano is synthesized, approved as the closest of five variants. See the [proposal](../design/electric-guitar-piano-proposal.md#results-part-6) |
| F18 | A strum spells out a pitch, velocity and onset offset per string, and each chord shape is written out. There is no strum or guitar-voicing shorthand | Open |
| F19 | The [producer guide](../ai-producer-guide.md) never mentioned `transpose` on `use`; the AI found it in the specification | Fixed: the guide now shows it |
| F20 | `analyze` cannot report gain reduction per section, so a glue or limiter flattening the chorus was found only by comparing region loudness | Open |
| F21 | A lead 11 dB under its accompaniment raised no finding. `buried_note` judges each part in its own strongest band, and no part is marked as the lead | Fixed: `maac analyze --lead` names the lead, and `lead_under` (profile `default/5`) warns when its median note sits 8.5 dB or more under the rest of its mix point. See the [proposal](../design/lead-part-proposal.md) |
| F22 | Raising the amp's `drive` also raises its output, so a drive automation needs a matching level automation | Fixed: `synth.drive/1` has a [`reference`](../instruments.md#saturation) that holds the level. The `std/studio/1.1.0` guitar uses it: its `drive` runs from 1 to 16 within 0.88 LU, and the song's lead dropped its level automation |
| F23 | Imports and nodes share one ID namespace, so a node cannot be named after an import | Open; the diagnostic is clear |
| F24 | `band_dominance` reports spurious changes at section edges, where one 100 ms block holds both sections | Open, informational only |
| F25 | Half of the draft-1 nylon melody's notes (64 of 132) read `unpitched` at the 0.03 aperiodicity limit, so `pitch_mismatch` did not check them | Open |

## What to build next, in order

1. **Instrument palette (F17).** This is the owner's lasting complaint after
   both trials. Candidates: an electric guitar with an amp stage, as a
   library instrument with a level-compensated drive (F22); an acoustic
   piano; and a review of the std/studio and std/acoustic timbres by ear.
   See the [proposal](../design/electric-guitar-piano-proposal.md).
2. **A lead role in the analysis (F21, F20).** Let the score mark a part as
   the lead, so `analyze` can compare it with the accompaniment per section.
   Add gain reduction per section for the dynamics processors. F21 is done,
   by a flag rather than a mark in the score; see the
   [lead part proposal](../design/lead-part-proposal.md). F20 remains.
3. **Strum and voicing shorthand (F18).** A language change; it needs its own
   proposal, like the [authoring density work](../design/authoring-density-proposal.md).
4. **Small fixes (F23–F25).** Section-edge blocks in `band_dominance`, the
   pitch check's coverage of plucked notes, and possibly a separate import
   namespace.
