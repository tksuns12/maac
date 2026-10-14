# T4: a third AI production, on release 0.3.0

**Date:** 2026-10-14. **Producer:** Claude, an AI model, working in this
repository. **Listener:** the project owner.
**Result:** [`examples/lantern/lantern.maac`](../../examples/lantern/lantern.maac),
"Lantern Street".

The [T3 trial](ai-production-trial-t3.md) produced "Morning Ferry" and
ranked what to build next. All of that list shipped in 0.3.0: the
std/studio/1.1.0 electric guitar and piano, `maac analyze --lead`, gain
reduction per section, strummed chords and voicings, and profile
`default/7`. T4 repeated the exercise on 0.3.0, in a new genre, to test
those additions and find the next limits. Friction numbers continue from T3.

## Brief and rules

- **Brief:** an original song with a sung-style melody. The AI chose upbeat
  pop at 100 bpm in D major: piano-led verses, strummed steel guitar from the
  first chorus, the melody on an electric guitar, and the last chorus a whole
  step up. Asked whether to change the genre or arrangement, the owner said
  that was a matter of taste and not for now.
- **Tools:** MaaC source written directly, and only `maac check`,
  `maac analyze` (profile `default/7`, with `--lead lead:out`),
  `maac query-events` and `maac deliver`. One exception: a short script
  outside the song computed the 28 points of the vibrato curve and attached
  the pitch curves to 18 notes. Writing those by hand is friction F28.
- **Judgment:** the owner listened to each version on a phone and gave the
  verdicts.

## Outcome

- **Form:** 52 bars at 100 bpm, 124.8 s plus a 4 s tail. Intro, two verses,
  three choruses, a bridge and an outro are marked as regions. The verse runs
  D–A–Bm–G–D–A–G–A and the chorus G–D–A–Bm–G–D–A–A; the bridge plays
  Em–G–Bm–A. The last chorus and the outro are placed 200 cents up, in E.
- **Parts:**
  - the std/studio piano: a left-hand root and a right-hand triad, both from
    one voicing per chord;
  - a strummed steel-string guitar from the first chorus, on an acoustic
    guitar defined in the song (version 2);
  - the melody on the std/studio/1.1.0 electric guitar, with bends and a
    vibrato written per note (version 2) and its `drive` automated from 5 in
    the verses to 12 in the last chorus;
  - std/studio bass, `drums` kit and pad.
- **Mix and master:** a reverb send for the piano, strums and lead, a glue
  compressor at ratio 1.5, then `fx.limiter/1`. The first chorus is 1.5 LU
  louder than the first verse in integrated loudness. The limiter takes about 1.7 dB
  on average in the choruses and up to 7.2 dB on peaks; the owner did not
  remark on it.
- **Delivery:** `maac deliver` passed its checks at −14.16 LUFS integrated and
  −1.10 dBTP true peak.
- **Analysis:** no findings in any version. The lead's median note sits
  between −0.6 and +4.5 dB against the rest of the mix, by section.
- **Owner's verdicts:**
  - On version 1: "Good." The melody was clearly audible and the balance
    fine, but the acoustic guitar sounded "a bit cheap", and the melody
    sounded "more like a synthesizer than an electric guitar".
  - On version 2: the acoustic guitar is "much better"; the melody is
    "good".

## What happened

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the song in 525 lines: piano, strums on the std/acoustic steel guitar, bass and pad from voicings and `use` per bar; three strum rhythms for every chord; a kit; the electric-guitar lead with a drive automation | `maac check` passed on the first try: 2,363 notes and 552 hits |
| Iteration 1 | `analyze --section regions --lead lead:out` | No findings. The master read −15.81 LUFS, the chorus only 1.0 LU over the verse, and the lead 2.2 dB under the rest in the chorus. The glue took 2.9 dB in the chorus against 2.1 in the verse. Fixes: glue ratio 2 → 1.5, limiter gain 2 → 3 dB, and a level automation on the lead per section |
| Delivery 1 | `maac deliver` | 128.8 s, −14.42 LUFS, −1.10 dBTP, checks passed |
| Verdict 1 | Owner: "Good"; the acoustic guitar is "a bit cheap" and the melody sounds like a synthesizer | — |
| Draft 2 | Read both library instruments. The std/acoustic/1.0.0 steel guitar is a `synth.pluck/1` at its defaults, `hardness = 1` and `position = 0`: every string starts from the same full-band noise burst, and velocity changes only the level. The electric guitar's `vibrato` is one 5.5 Hz LFO on every note from note-on. Fixes: an acoustic guitar defined in the song (F27); the lead's `vibrato` at 0, with a delayed upward vibrato and whole-step bends as per-note pitch curves on 18 notes (F28); the lead's drive raised from 3 to 5 in the verses | `maac check` passed |
| Iteration 2 | `analyze --section regions --lead lead:out` | The new guitar's octave bands at first barely moved: 8 kHz sat 6.5 dB under the 1–2 kHz peak, against 5.2 dB before. A softer pick and stronger body resonances put 8 kHz 9.1 dB down and lifted 250 Hz by 2 dB; the level was matched to version 1. No findings. 41 of the lead's 178 notes, the bent ones, were not pitch-checked (F29) |
| Delivery 2 | `maac deliver` | 128.8 s, −14.16 LUFS, −1.10 dBTP, checks passed |
| Verdict 2 | Owner: the acoustic guitar is "much better"; the melody is "good" | — |

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- `maac check` accepted the 525-line first draft and every later edit.
- One voicing per chord served both of the piano's hands, through
  `bottom = 1` and `top = 3`.
- Three strum patterns served every chord in the song. Before 0.3.0, each chord
  shape would have been written out per rhythm (F18).
- `--lead` gave the lead's margin per section, which set the level
  automation. The glue's `dynamics` took 0.8 dB more in the chorus than in the
  verse, which pointed to the gentler ratio (F21, F20).
- The electric guitar held its level as its `drive` moved from 5 to 12 (F22).
- Per-note pitch curves worked on a library instrument without changes to it.
- Profile `default/7` raised no false alarms, and delivery passed on the first
  run each time.
- The piano, in a song for the first time, drew no complaint.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F26 | The chord progression is restated in every part. A voicing belongs to one instrument's range, so each part names its own voicing bar by bar: 91 of the 595 lines are the `use` lines that do so (piano 27, strums 20, bass 24, pad 20). The bass follows the chords through `transpose` values in cents worked out by hand (A −500, B −300, G −700, E +200) | Open |
| F27 | The std/acoustic/1.0.0 guitars start every string from the same full-band noise burst, at any velocity, through one 450 Hz low-pass for a body. Strummed in a full mix, the owner heard them as "a bit cheap". The pick hardness and pick position that `synth.pluck/1` gained on 2026-10-11 came after that library was frozen. The AI defined an acoustic guitar in the song, in 47 lines: velocity on pick hardness, the pick a fifth of the way along the string, and body resonances at 105 and 215 Hz. The owner: "much better" | Out of scope. The owner sees the built-in libraries as a language's standard library: a baseline, not the best sound available. The language handled this case: a song, or a [library](../instruments.md) imported by `path` and `hash`, can define a better instrument from MaaC's nodes, as this song did |
| F28 | The electric guitar's `vibrato` is one LFO at a fixed depth on every note from note-on, rising and falling about the pitch, like a synthesizer's; the owner heard the lead as one. Per-note pitch curves fixed it: a vibrato that starts 300 ms after the pick and only rises from the pitch, as a bent string does, and bends from a whole step below. But a vibrato curve is 28 points, which the AI computed by script; a new rate or depth means recomputing them | Open |
| F29 | `pitch_mismatch` skips every note with a pitch expression. The 41 lead notes left unchecked are the bent and held ones, the most exposed in the song | Open |

## What to build next, in order

The owner set the scope after the trial: the built-in instrument libraries
are like a programming language's standard library. They should be usable,
not the best sound available, and improving them is not this project's main
work. F27 is therefore out of scope, and the list keeps to the language and
its tools.

1. **Guitar vibrato and bends (F28, F29).** Let a vibrato be written in a few
   values: a delay and a one-sided shape on the instrument's vibrato, a
   repeating segment in a curve, or a vibrato expression with a rate, depth
   and delay. Check an expressed note's pitch against its curve. See the
   [vibrato proposal](../design/vibrato-proposal.md).
2. **Progressions written once (F26).** A chord progression declared once,
   which each part plays through its own voicings, with a bass that follows
   the chord roots. A language change; it needs its own proposal, like the
   [strum and voicing proposal](../design/strum-voicing-proposal.md).
