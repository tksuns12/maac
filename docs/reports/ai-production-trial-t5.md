# T5: a fourth AI production, in electronic dance music

**Date:** 2026-10-14. **Producer:** Claude, an AI model, working in this
repository. **Listener:** the project owner.
**Result:** [`examples/afterlight/afterlight.maac`](../../examples/afterlight/afterlight.maac),
"Afterlight".

T2 to T4 produced lo-fi hip-hop, folk-pop and pop: mid-tempo songs on band
and acoustic instruments, led by a melody. After T4 every finding was fixed
or put out of scope, so T5 tested a major genre none of them touched. It
also had a question to answer. Glide synthesizers in electronic music, and
808 slides in trap, play one line that slides from note to note without a
new attack. MaaC has no legato: every note is its own voice. T5 was to
settle whether that is a missing language feature or a convenience.
Friction numbers continue from T4.

## Brief and rules

- **Brief:** electronic dance music, the owner's choice. The AI chose
  progressive house at 126 bpm in F minor, with a sidechain pump, a build
  with a snare roll and a riser, two drops, and a lead that glides between
  some notes, to test legato.
- **Tools:** MaaC source written directly, and only `maac check`,
  `maac analyze` (profile `default/8`, with `--lead lead:out`),
  `maac compile` and `maac render` for excerpts, and `maac deliver`. Two
  scripts outside the song served the listening rounds: one wrote the
  legato variant of the glide excerpt, and one measured how much the mix
  pumps (see F33).
- **Judgment:** the owner listened to each version on a phone and gave the
  verdicts.

## Outcome

- **Form:** 84 bars at 126 bpm, 160 s plus a 4 s tail: intro, groove,
  breakdown, build, drop, interlude, a second build, a second drop and an
  outro, each a region. One four-bar loop, Fm–Db–Ab–Eb, runs under the
  whole track.
- **Parts:**
  - five instruments defined in the song from MaaC's synthesis nodes: a kick
    (a sine dropping from about 220 Hz to F1, a click and a drive, 37 lines),
    a supersaw (seven saws detuned by up to 30 cents and spread across the
    stereo field, 53 lines) for the chords and the lead, a saw bass with a
    sub (36), a pluck for the arpeggio (33) and a stereo noise riser (28);
  - from the libraries, the std/studio `drums` kit for claps, hats, snare
    rolls and crashes, the piano for the breakdown and the pad.
- **Mix and master:** every part but the kick, kit and riser runs through
  one bus: a high-pass that the builds sweep up, a low-pass that opens the
  intro and closes the outro, and an `fx.compressor/1` keyed by the kick
  through its `sidechain` input, which pumps the bus. A reverb send and a
  dotted-eighth `core.delay/1` with darkened feedback, a glue compressor at
  ratio 2, then `fx.limiter/1` at +12.5 dB.
- **Delivery:** `maac deliver` passed its checks at −11.36 LUFS integrated
  and −1.09 dBTP true peak.
- **Analysis:** no warnings in the approved version. The lead's median note
  sits 0.8 to 0.9 dB under the rest of its mix point in the drops.
- **Owner's verdicts** (translated):
  - On version 1: "Oh, it's really good." The build is "so good", and the
    supersaw and the kick are good. But the drop did not feel pumped, and
    the glides were "not choppy, but not sliding either".
  - On two glide excerpts: A, the retriggered glide, "feels more like a
    glide" than B, the legato one.
  - On two pump strengths: "1 is better, but 2 isn't bad either. 1 isn't
    overdone. With the pumping in, it's so exciting and good."
  - On version 2: "Oh, much better." The drops have "a feeling of bursting
    out". "This is fine as it is."

## What happened

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the track in 706 lines: five instruments, harmony and voicings for the bass, chords, piano and pad, four arpeggio patterns, the kit, snare rolls, a riser, the lead melody with glides and vibrato, the bus chain and a delivery | `maac check` passed on the first try: 2,856 notes and 733 hits |
| Iteration 1 | `analyze --section regions --lead lead:out` | `lead_under` in three sections: the lead 9.5 to 11.6 dB under. The build, at −8.7 LUFS short-term, was 5.6 LU louder than the drop: thirty-second snares pile up. The pump took 1.3 dB on average in the drop. Fixes: lead ×4, chords ×2.5, the snare roll's level ramped from 0.2 to 0.8, the pump's threshold from −20 to −28 dB |
| Iterations 2–5 | Section balance and loudness | No warnings. The breakdown and interlude played louder than the drops; section level automations on the chords, lead and roll fixed that. The limiter's gain went from 4 to 11 dB for −10.8 LUFS. Driving the kick harder did not lower its crest factor (12.8 dB at drive 2, 12.6 at 6): its peaks come from its envelope, not its waveform |
| Delivery 1 | `maac deliver` | 164 s, −10.85 LUFS, −1.09 dBTP, checks passed |
| Verdict 1 | Owner: "really good"; the build "so good"; no pumping heard; glides "not sliding" | — |
| Glide round | Two excerpts of the drop's first eight bars, both with the glide lengthened from 80 ms to a sixteenth (5/42 s): A retriggers each note as before; B merges each glide note with the one before it into one note, so the voice slides without a new attack. B needs its slide in seconds from the merged note's start (5/21 s, 5/14 s, 10/21 s) and a vibrato per position (delays of 41/84 s and 17/28 s) | Owner: A "feels more like a glide". Legato did not help; A goes into the song |
| Pump round | Measured the master above 300 Hz, folded on the beat: the level swings 6.1 dB per beat. Deeper pump settings barely changed it (6.8 and 6.9 dB); the unpumped lead and drums covered the pumped chords. With the lead through the pump, it swings 14.9 and 17.9 dB. Two excerpts at those strengths | Owner: 1 (14.9 dB) is better, and "so exciting" |
| Version 2 | The lead through the pump at the first strength. The pump lowers the drops' average level, so the breakdown and interlude were lowered under them (pad, piano, chords, lead and a sustained bass), and the limiter's gain went to 12.5 dB | No warnings. The drops measure −11.0 LUFS against −12.7 for the breakdown |
| Delivery 2 | `maac deliver`, with the loudness range widened from −11…−9 to −12…−10 rather than limit the kick harder | 164 s, −11.36 LUFS, −1.09 dBTP, checks passed |
| Verdict 2 | Owner: "much better"; the drops burst out; "fine as it is" | — |

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- `maac check` accepted the 706-line first draft and every later edit.
- A genre the libraries do not cover was built in the song: the owner
  approved the kick and the supersaw on first hearing. This bears out the
  scope decision after T4: the language, not the library, has to reach the
  sound.
- One `harmony` served four parts: the bass, the supersaw chords, the
  piano's left hand and the pad.
- The build, which the owner singled out, took no new feature: `count` and
  `stretch` wrote each snare roll in four lines, an automation ramped the
  snare's level, one riser note was stretched to both builds under a
  `normalized` gain curve, and `fx.eq/1` automations swept the bus.
- The sidechain compressor worked as specified: kick into `sidechain`, and
  `dynamics` reported what it took per section.
- `maac compile` and `maac render --start-frame --end-frame` gave 16-second
  excerpts for the listening rounds, so each round compared one change.
- Rational seconds such as `5/42s` gave a sixteenth at 126 bpm exactly.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F30 | A glide starts at the previous note's pitch, so its curve depends on the interval: the melody's five glides need four curves (from a fourth below, a third below, a step below and a step above). A changed note means re-checking which curve its glide uses | Open; a convenience. The trial's question is answered: a glide does not need legato. In a blind round the owner preferred a retriggered glide to a legato one, and the legato version took merged notes, positions worked out in seconds and a vibrato per position |
| F31 | An arpeggio cannot follow the harmony. A chord leaf plays all its members or the outer ones (`bottom`, `top`); it cannot pick the second or third. The arpeggio is four patterns, one per chord, so the progression is written out again in 32 notes, and a new harmony would not move it | Open. The [harmony proposal](../design/harmony-proposal.md#not-proposed) deferred picking a middle member for a walking bass's fifth; a second genre now needs it |
| F32 | A tempo-synced delay is a frame count worked out by hand: a dotted eighth at 126 bpm is 17,142.857 frames, written as 17,143. It would not follow a tempo change | Open; a convenience |
| F33 | `maac analyze` cannot tell whether a pump is heard. The keyed compressor took 4.3 dB on average in the drops, up to 13.9, yet the owner heard no pumping. Measured outside the analyzer, the mix swung only 6.1 dB per beat, and deeper settings barely changed that: the parts that carried the drop were not pumped | Open: a measurement of how much the mix's level swings on the beat, per section |
| F34 | The track uses 8.66 billion of the song profile's 10 billion execution-work units (87 %), mostly in the seven-saw voices. At this density the limit is reached at about 3:10, so a five-minute extended mix, common in dance music, cannot be built. The earlier trial songs are as close: "Lantern Street" uses 8.88 billion in 2:09 (89 %), "Morning Ferry" 5.99 billion in 2:17 (60 %) and "Late Window" 3.25 billion in 1:18 (33 %). At "Lantern Street"'s density the limit is about 2:25, shorter than most pop songs. The default profile refuses even `maac compile` without `--profile song` | Open. A resource policy, not a language rule: the delivery took 83 s of CPU |

## What to build next, in order

The trial answered its question: legato is not a missing language feature.
A retriggered note that slides from the previous pitch, over a sixteenth,
was preferred by the owner to a true legato slide. The list keeps to what
blocked or slowed this track.

1. **Room for full-length tracks (F34).** The song profile's work limit
   stops this dance track at about three minutes, and T4's pop song at
   about two and a half. Raise the limit, or base it on measured render
   time, so that a normal-length track fits. A resource policy, not a
   language change.
2. **Picking chord members (F31).** Let a chord leaf play one member of the
   supplied voicing by its position, so an arpeggio or a walking bass
   follows the harmony. A language change, deferred twice and now needed by
   two genres.
3. **Measuring the pump (F33).** Report how far the mix's level swings on
   the beat, per section, so a pump that nobody can hear is found before the
   listener finds it. A tool change.

F30 and F32 are conveniences and wait until another track needs them.
