# T8: a seventh AI production, a gayageum sanjo

**Date:** 2026-11-02 to 2026-11-04. **Producer:** Claude, an AI model,
working in this repository. **Listener:** the project owner. **Result:**
"Rain on the Eaves" (처마 끝 빗소리), version 3
([`examples/sanjo/sanjo.maac`](../../examples/sanjo/sanjo.maac)). After two
rounds on the gayageum's sound, the owner turned the trial to the language
alone; a third round chose a steady quickening over a computed push and
pull.

T2 to T7 covered lo-fi hip-hop, folk-pop, pop, dance music, an orchestra
and jazz, all in Western meters and tunings. T8 tested Korean traditional
music, the owner's choice: sanjo, a solo that starts slow and gathers speed
through a chain of jangdan, the rhythmic cycles, over a janggu drum. No
MaaC library has a gayageum or a janggu, so both were built in the song.
Friction numbers continue from T7.

## Brief and rules

- **Brief:** Korean traditional music, the owner's choice. The AI chose a
  condensed gayageum sanjo of about two and a half minutes, in gyemyeonjo
  on A: mi trembles widely, la is plain, and do bends down to si, slowly
  when the line falls and quickly when it rises. Four jangdan, each a
  region: jinyangjo, jungmori, jajinmori and hwimori, then a last stroke.
- **Tools:** `maac check`, `maac analyze` (profile `song`,
  `--section regions`), `maac render` for clips, and `maac deliver`. The
  source was written directly in MaaC, with no script writing notes. Python
  outside MaaC measured the gayageum's attack and tail, and in version 4
  computed the tempo map (F66).
- **Judgment:** the owner listened on a phone. Each round was blind: pairs
  of clips at −20 LUFS (the ending at −18), the letters swapped between
  pairs where there were two, and revealed after the verdict.
- **Direction:** on 2026-11-03 the owner said the recent work felt like
  building basic instruments rather than the language. From then on, the
  gayageum stayed as it was and the trial looked only for what the
  language could not say.

## Outcome

- **Form:** 243 quarter notes, about 135 s plus a 3 s tail. Jinyangjo,
  two jangdan of four gak in 18/8 (bars 1–8), quickening from 67.5 to 75
  quarter notes a minute, a dotted-quarter beat of 45 to 50; jungmori, five
  jangdan in 12/4 (9–13), 84 to 96; jajinmori, twelve jangdan in 12/8
  (14–25), 150 to 210; hwimori, six jangdan in 12/8 (26–31), 225 to 247.5;
  and a last stroke that broadens to 180.
- **Parts:** 260 notes and 197 hits in 523 lines.
  - The gayageum: a bank of 24 harmonic modes in `synth.modal/1`, whose
    higher modes die sooner, struck by a 5 ms burst of per-note noise at a
    point that `synth.random/1` moves by up to 1/40 around 1/8, with a
    finger's snap and a board resonance at 280 Hz.
  - The janggu: a kit of two instruments built in the song. The left head,
    a low "kung" of three sines whose pitch sinks, and the palm's thump;
    the right head, a stick's crack of filtered noise and two short rings.
    덩, both heads together, is two hits at one time.
  - The left hand: 40 of the 151 written notes carry an ornament chosen
    for that note. Four `vibrato` declarations with `shape = up` tremble
    mi; two curves bend do to si; one pushes mi up from a whole tone below.
  - Dynamics: `phrase`s on two shared curves shape the first three
    jangdan; a `vary` for the hands and one for the sticks.
- **Mix and master:** the two parts into one sum, a 1.6 s `fx.reverb/1`
  room returned at 0.2, a glue compressor at 3:2, a trim of 1.4, then
  `fx.limiter/1` with a −1.1 dB ceiling.
- **Analysis of version 3:** sections at −19.4 (jinyangjo), −18.1, −14.5
  and −13.3 LUFS, louder as the music quickens. The janggu sits 12 to 14 dB
  under the gayageum in the 500 Hz band. The room is 3.01 dB heavier on the
  left in hwimori, following the gayageum's pan. Analysis took 29 s.
- **Delivery:** `maac deliver` passed at −16.16 LUFS and −1.10 dBTP in
  29 s.
- **Owner's last word** (translated), on round 3: "a somehow felt natural.
  I'd say it was the vibrato I liked."

## What happened

Verdicts are translated.

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the four jangdan, the ornaments, the gayageum on `synth.pluck/1` and the janggu | F64; F37 (a pattern and a region both named `jinyang`, and the other three) |
| Calibration | The gayageum and the janggu alone at velocity 0.7, set by hand to about −20 LUFS | F42 |
| Version 1 | Analysis, then delivery | −15.38 LUFS, −1.10 dBTP |
| Verdict 1 | Owner: "It's a fair imitation, but there's an oddly metallic sound at the end" | F65 |
| Diagnosis | Stems of the ending, then seven pitches alone | The gayageum alone carried it: its low notes kept harmonics 2 to 6 up to 9 dB above the fundamental 0.6 s after the pluck |
| Version 2 | The string as a `synth.modal/1` bank, struck by a 0.5 ms pulse | The tail's harmonics fell away; −16.53 LUFS |
| Round 1 | Version 1 against version 2: the slow opening and the ending | Owner: "In the slow one, a is definitely convincing, but in the ending a is metallic and b is just... like a synthesizer." Version 2 won the opening, and lost its metal in the ending, where fast notes are mostly attack, each the same filtered pulse |
| Version 3 | The pulse replaced by a burst of per-note noise, and the strike point moved per note | −16.16 LUFS |
| Direction | Round 2, version 2 against version 3, was ready | Owner: "Lately it feels like we keep making basic instruments, not the language." Round 2 was not played; the gayageum stays at version 3 |
| Version 4 | Version 3 with each gak of jinyangjo pushing and pulling over the quickening, 31 tempo points computed by a script | F66 |
| Round 3 | Version 3 (a) against version 4 (b), the first 40 s | Owner: "a somehow felt natural. I'd say it was the vibrato I liked." Version 3 is the result |

Two lessons for the next trial:

- **A listener's image names an impression, again.** The owner credited
  the vibrato, but the two clips' vibratos are identical; only the tempo
  differed. As in T7, the blind pair, not the words, told what changed.
- **The first clip won this time.** In T7 the second clip won 11 of 13;
  here the first won the one pair that decided anything. One pair proves
  little, but it argues against a fixed lean to the second.

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- The whole piece was written by hand, with no script writing notes: each
  line with `at = after(...)`, and the jangdan's hits as patterns used
  inside patterns. T7's answers carried over to music they were not
  designed for.
- The ornaments of a tradition outside Western practice were shared,
  named declarations: a trembling that only rises, as the left hand only
  presses, is a `vibrato` with `shape = up`; a slow bend that follows the
  note's length is a normalized curve, a quick one a seconds curve.
- Four meters, 18/8, 12/4 and 12/8, and four ramps lived in one meter map
  and one tempo map, and each jangdan was a region that analysis reported
  by name.
- Every change to the gayageum's sound used nodes MaaC already had:
  `synth.modal/1`, per-note noise and `synth.random/1`. None of it was a
  language limit.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F64 | A tempo map cannot jump. Its points strictly increase ([§5.1](../../MaaC-1-Specification.md#51-tempo-map)), and a linear segment ramps to the next point's tempo, so a jangdan that quickens to its end cannot hand over to a faster one at the same instant. Version 3 ends each ramp a quarter note early and holds its top tempo for that quarter. A hold of 1/128 of a quarter, at most about 6 ms here, also checks, so the jump can be written to within milliseconds | Open; small. A second point at the same position could mark a jump |
| F65 | The gayageum's notes ended with a metallic ring. `synth.pluck/1`'s loss filter takes too little from the low notes' upper harmonics | Not a language finding: existing nodes fixed it. Instrument realism belongs to a standard-library track |
| F66 | A push and pull cannot be laid over a quickening. The tempo map holds absolute tempos, with no repetition and no layering, so a script computed each point as the ramp times a factor: 31 points such as `14443/240bpm`, one line of 1073 characters. The source keeps neither the ramp nor the push and pull, and changing either means running the script again | Open; no evidence of need. Round 3 preferred the steady ramp. If a later trial needs it, a relative tempo curve that repeats as a `phrase` does would keep the intent in the source |

F37 and F42 recurred. F37: `E_DUPLICATE_ID`, a pattern and a region both
named for their jangdan, a third trial in a row. F42: both instruments
calibrated by hand.

Not proposed: the ornaments depend on each note's length and place in the
line, so no rule by pitch would replace writing them per note. Sanjo's
tuning sits within a few cents of equal temperament here, so the piece did
not test `tuning` or `degree`.

## What to build next, in order

The owner decides; this is the AI's recommendation.

1. **Nothing from T8 is urgent.** Sanjo was written by hand without a
   language change. F64 has a close workaround and is a small
   convenience; F66 waits until a trial needs it.
2. **Effects outside instruments.** Sanjo needed one room, a glue
   compressor and a limiter, so T8 did not test the mix. A review of
   production on 2026-11-03 found the mix is where the language is
   thinnest. Outside an instrument, a project has a fixed set of
   processors, and the `synth.*` nodes that could build a saturator, an
   auto-pan or a modulated delay are allowed only inside instruments,
   whose shared graph hears only their own voices. `core.pan/1` takes mono
   only, so a stereo instrument cannot be panned (`E_PORT_TYPE`), and
   `core.delay/1` counts frames, so an echo cannot be written in beats
   (`E_UNIT`) and drifts when the tempo moves. A proposal can start from
   what cannot be written, checked on the existing songs, and a next trial
   in a genre that leans on the mix would set its order. The
   [effects proposal](../design/effects-proposal.md) does this.
3. **The gayageum's sound,** if the owner wants it, in a standard-library
   track separate from the trials.
