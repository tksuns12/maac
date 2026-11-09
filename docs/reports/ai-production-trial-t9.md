# T9: an eighth AI production, a roots dub

**Date:** 2026-11-05 to 2026-11-09. **Producer:** Claude, an AI model,
working in this repository. **Listener:** the project owner. **Result:**
"Low Tide", version 3
([`examples/dub/dub.maac`](../../examples/dub/dub.maac)). The riddim was
written as easily as earlier trials' music. The dub, the performance on
the desk, was not. Every desk move is one curve over the whole song at
positions counted by hand (F68). The tape echo's feedback loop had to be
built from six nodes in the project graph, because an effect cannot hold
a loop (F70).

Release 0.5.0 put effects outside instruments, and T8's list and the
[effects proposal](../design/effects-proposal.md) asked for a trial in a
genre that leans on the mix. The AI proposed dub, and the owner agreed:
"Let's go that way." A dub is a mix played live on the desk over a
riddim, a repeating backing track. Parts drop out and back in. Single hits
are thrown into a tape echo whose repeats darken and swing across the
stereo field. The echo is let run until it nearly screams, a filter
sweeps the drums, and a siren cries over the top. Friction numbers continue
from T8.

## Brief and rules

- **Brief:** a roots dub of about three minutes in G minor at 74 bpm, in
  4/4 with a one-drop beat, over a riddim of | Gm | Gm | Eb | F |. The
  question was whether MaaC can write the desk's performance, not only
  the riddim: the drop-outs, the throws, the runaways and the sweeps.
- **Tools:** `maac check`, `maac analyze` (profile `song`,
  `--section regions`), and `maac deliver`. The source was written
  directly in MaaC, with no script writing notes. Python outside MaaC
  measured the guitar's chords in versions 1 and 2, and each pass of the
  echo in the probes for F70.
- **Judgment:** the owner listened on a phone, with blind pairs at
  −20 LUFS as in T8.
- **Direction:** T8's rule held from the start. Instruments are made good
  enough quickly, and the trial looks only for what the language cannot
  say. The drums, snare, bass and electric guitar are std/studio/1.2.0's
  and the organ std/basic/1.1.0's. A melodica and a dub siren were built
  in the song.

## Outcome

- **Form:** 224 quarter notes (56 bars) at 74 bpm, 181.6 s plus an 8 s
  tail. There are seven sections, each a region:
  - intro (0q): drums through a low-pass that opens, and the siren;
  - riddim_a (16q): the full riddim and the melodica;
  - dub_1 (48q): bass and guitar drop out, throws, a runaway;
  - riddim_b (80q): the full riddim and the melodica's answer;
  - dub_2 (112q): drums and bass, a high-pass sweep, the echo's speed
    changes, the siren;
  - riddim_c (160q): steppers, with the kick on every beat;
  - outro (192q): the parts leave one by one, and the last chord goes
    into the echo.
- **Parts:** 1387 notes and 464 hits, from 95 notes, 25 chords and 17
  hits written in patterns, in 531 lines.
  - The riddim:
    - one-drop and steppers patterns on the std/studio kit, with the snare
      on a node of its own (F69);
    - a two-bar bass line;
    - the guitar's skank, short chords on beats two and four, and the
      organ's bubble, soft chords on the off-beat eighths, both from one
      `harmony` through two `voicings`;
    - a lilting groove and a `vary` on everything.
  - The melodica is a saw, a square and a sine through a small body. The
    siren is a triangle that an LFO wobbles and a pitch curve sweeps up an
    octave.
- **The desk:**
  - **Effects:**
    - `sweep`, a low-pass and a high-pass behind controls, on the drum
      bus;
    - `thin`, a 600 Hz high-pass, on the guitar;
    - the tape echo, in three effects:
      - `tape`, a `synth.delay/1` on the score clock at a dotted eighth;
      - `wear`: a drive, a 300 Hz high-pass and a 3200 Hz low-pass;
      - `spread`: a balance and a width of 1.3.
    - In the project graph, a `core.matrix/1` that partly swaps the
      sides, one frame of `core.delay/1` and a `core.gain/1` close
      `tape` and `wear` into a loop.
    - A score-clock `core.lfo/1` swings the returns' balance once every
      two bars.
    - A spring, a 2.2 s `fx.reverb/1`, takes the snare and the siren.
  - **Moves:** 11 curves of 99 points under 11 automations, in 56 of the
    531 lines:
    - the intro's opening low-pass and dub 2's high-pass sweep;
    - drop-outs for the bass, the guitar and the organ;
    - 27 throws: 10 of the snare, 5 of the guitar and 12 of the melodica;
    - three runaways of the echo, each cut back;
    - the tape speeding up from a dotted eighth to an eighth in dub 2,
      and slowing back.
- **Mix and master:** a glue compressor at 2:1, a trim, then
  `fx.limiter/1` with a −1.1 dB ceiling.
- **Analysis of version 3:**
  - Sections measured −21.8 LUFS (intro), then −16.6, −18.5, −16.6,
    −18.6, −16.5 and −17.8. Each dub sits about 2 dB under the riddims
    around it.
  - Sources: bass −18.4 LUFS, drums −20.5, melodica −21.0, guitar −23.2,
    organ −25.2 and siren −25.7. The echo's return measured −26.8 over
    the song, from −24.9 to −30.7 by section.
  - The analysis warned that the echo's return leans 3.19 dB right in the
    intro. It also warned that each siren note sounds 163 cents above its
    written pitch; the note sweeps up an octave by design.
- **Delivery:** `maac deliver` passed at −17.42 LUFS and −1.10 dBTP in
  49 s.
- **Owner's verdict:** version 3 was sent to the owner on 2026-11-09;
  none yet.

## What happened

Verdicts are translated.

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the riddim, the melodica and the siren, and the desk as effects, faders, sends and curves over the whole song | F67, F68, F69 |
| Calibration | Each part's level set by hand over two passes | F42 |
| Version 1 | Analysis, then delivery | −17.45 LUFS, −1.10 dBTP |
| Verdict 1 | Owner: "The guitar sounds like a wind instrument, doesn't it?" | |
| Diagnosis | Measured the guitar's chords alone | Each chord rang 258 ms after its peak, with 65% of its energy at 0.5–2 kHz and none above 6 kHz: a horn section's stab on two and four |
| Version 2 | Brightened and choked the chords with the guitar's existing controls, shorter chords, a higher voicing and the `thin` effect | 115 ms, 70% of the energy at 2–6 kHz. Sound design with existing nodes, not a language finding |
| Round 1 | Prepared version 1's guitar against version 2's in riddim_a, blind | Owner: "But here we are talking about instruments again. We agreed not to tune instruments anymore." Round 1 was withdrawn unheard; the instruments stay at version 2 |
| Probes | Eight small documents tested the desk's limits | F67 located; F68 confirmed; F70 |
| Version 3 | Version 2 with the tape loop of F70 in the project graph, two more snare throws to feed the first runaway, and each throw point raised by hand | −17.42 LUFS, −1.10 dBTP; F68 |

Two lessons for the next trial:

- **The AI broke the rule it had been given.** T8 ended with the owner
  turning the trial to the language alone. In T9, one remark on the
  guitar brought a measurement, a change of settings and a blind round
  that nobody had asked for. From now on, a remark on an instrument's
  sound during a trial is logged, and the instrument stays as it is,
  unless the fix needs something MaaC cannot say.
- **A dub is written in two places.** The riddim is patterns, harmony and
  placements, as in earlier trials. The dub, meaning which hit is thrown
  and when a part drops, lives in curves at absolute positions, far from
  the notes. Each later change to the music had to be made in both places
  by hand: adding throws to feed a runaway, and raising the throws' level.

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- Effects outside instruments did what the effects proposal promised. A
  sweep ran on the drum bus and a high-pass on one part. The tape's echo
  time, written in quarter notes, was automated from a dotted eighth to
  an eighth and back with no frame arithmetic. That is T5's F32, closed
  in 0.5.0, now in use.
- A score-clock LFO swung a stereo return's balance, which `core.pan/1`
  could not do before 0.5.0.
- One `harmony` gave the guitar and the organ their chords through two
  `voicings`, and the riddim's 16-quarter loop was placed, not copied.
- With one frame of `core.delay/1`, the project graph holds a feedback
  loop through effects. All three dub echoes could be written and
  measured:
  - repeats that darken and thin with each pass: the spectral centroid
    falls from 4026 to 1538 Hz, and the energy below 300 Hz from 22.7% to
    0.4%;
  - a runaway above unity gain, held by the drive near −8 dBFS;
  - repeats that alternate between left and right.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F67 | An event that fails a timing check is not named. `maac check` said "E_INTERVAL: resolved onset lies outside score interval at events", and `--json` gave only `"path": "events"`. A `vary` of 6 ms on the drums placed at 0q had moved the first hat before the score's start, and the cause was found by removing each `vary` in turn. [§23](../../MaaC-1-Specification.md#23-validation-and-diagnostics) says an event failure names the note, hit or message that produced it. `schedule_view` in `src/plan_v3.rs` has the event in hand when it raises this error and the two checks beside it | Fixed after the trial: the error names the note or hit, and its message begins with the event's address, such as `drums_intro/0/h1` |
| F68 | A desk move cannot be written with the music. [§12](../../MaaC-1-Specification.md#12-curves-and-parameter-automation) allows one global automation per parameter, so each send and each fader is one curve over the whole song. Each throw is two step points at positions counted by hand: the guitar's chord on beat four of bar 16 is `(63q, 2, step), (253/4q, 0, step)`. A throw cannot be named once and used twice: the same curve at 6q and 14q on one send is `E_AUTOMATION_WRITER`. It cannot be written in its section, or carried by a pattern or a placement, so moving `reed_b` would leave its throws behind. Raising the throws' level meant editing 15 points one by one. The other way, a second node that plays copies of the thrown notes into the echo only, plays a different hit under `vary`: the copy of bar 2's snare is 2.01 ms late at velocity 0.791, the dry hit 1.37 ms at 0.807 | Open; the main finding |
| F69 | A kit has one output. To throw only the snare, it left the kit for a node and patterns of its own (`drop_snare`, `drop_snare_ghost`), while the kick and hats stayed in the kit, so one drum part became two placements. A kit's pieces have their own `level` and `pan`, but not their own outputs or sends | Open; the split snare works |
| F70 | A feedback loop cannot be part of an effect. Inside an effect, a cycle is `E_ALGEBRAIC_LOOP`, and an effect node cannot break a cycle in the project graph. `synth.delay/1`'s own feedback stops at 0.99, and only its damping acts inside the loop. Its repeats therefore all wear the same, and the echo cannot run away. With one frame of `core.delay/1` the project graph holds the loop, as above. But each song that uses it needs 3 effects, 6 nodes and 6 connections, and each pass is one frame longer than the echo's time. None of it can be exported from a library as one tape echo | Open |

F42 recurred: every part's level was set by hand.

Not pursued: the siren's 163 cents in the analysis. The note sweeps up an
octave on purpose, and the AI did not check what the analysis compared it
with. Version 2's guitar used existing controls only, so instrument
realism stays with a standard-library track.

## What to build next, in order

The owner accepted this order on 2026-11-09.

1. **Desk moves that travel with the music (F68).** A dub, like any mix
   that plays parts in and out, needs a move written once and used where
   the music is. A throw could be named as a `vibrato` or a `phrase` is,
   and placed with a hit, a pattern or a placement. Several moves on one
   parameter would then combine instead of conflicting. A proposal can
   start from version 3's 99 points and the `reed_b` case. The
   [desk moves proposal](../design/desk-moves-proposal.md) does this.
   F67, a small bug, was fixed after the trial.
2. **Feedback inside effects (F70).** An effect that holds a loop, broken
   by a delay, so that a tape echo that wears and runs away is one
   library effect.
3. **Per-piece kit outputs or sends (F69),** when a later trial needs
   them. The split snare works.
4. **The instruments' sound,** if the owner wants it, in a
   standard-library track separate from the trials.
