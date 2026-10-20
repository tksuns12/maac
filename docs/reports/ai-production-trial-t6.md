# T6: a fifth AI production, an orchestral film cue

**Date:** 2026-10-15 to 2026-10-20. **Producer:** Claude, an AI model,
working in this repository. **Listener:** the project owner.
**Result:** "Homeward", version 1. Version 2 waits on the
[bowed strings](../design/bowed-strings-proposal.md), and its source will
join the examples with it.

T2 to T5 produced lo-fi hip-hop, folk-pop, pop and dance music, all on band
or electronic instruments. T6 tested an orchestra, the owner's choice: a
film cue. No MaaC library has an orchestral instrument, so every part was
built in the song. The trial also returned to T5's question about legato
from the other side. A synthesizer did not need it; a bowed string does.
Friction numbers continue from T5.

## Brief and rules

- **Brief:** an orchestral film score, the owner's choice. The AI chose a
  two-minute cue, "Homeward", that moves from D minor to D major. A horn
  sings the theme over strings and harp, the violins take it up an octave
  higher, a build quickens to a climax with brass, timpani and cymbals, and
  a flute closes it alone.
- **Tools:** MaaC source written directly, and `maac check`, `maac analyze`
  (profile `default/9`, `--section regions`, `--lead horns:out`),
  `maac compile`, `maac render` and `maac deliver`. After the first verdict,
  listening rounds used short clips. The first was written in MaaC. MaaC
  could not express what the strings needed, so the later ones came from a
  Python prototype outside it.
- **Judgment:** the owner listened to each version on a phone and gave the
  verdicts.

## Outcome

- **Form:** 38 bars, 127.6 s plus a 6 s tail: intro (bars 1–4), theme
  (5–12), the theme again (13–20), build (21–24), climax in D major (25–32)
  and coda (33–38), each a region. The tempo is 76 bpm. It quickens to 84 in
  the build, then drops to 72 for the coda and slows to 54.
- **Parts:** seven instruments defined in the song, in 329 lines, played by 15
  nodes:
  - a string section of six detuned saws, two vibratos, rosin noise and a
    low-pass (70 lines), for violins, violas, cellos and basses, long and
    short;
  - a brass section of two saws and a square with a scoop and a filter
    envelope (48), for horns, trumpets and trombones;
  - a flute of a sine, an octave above and a triangle, breath noise, a chiff
    and a vibrato (50);
  - a harp on `synth.pluck/1` (30);
  - timpani: a noise mallet into two modal banks and a sine (40);
  - a suspended cymbal (33) and a bass drum (32).
- **Mix and master:** all fifteen nodes into one sum, with a 2.8 s
  `fx.reverb/1` hall returned at 0.35. A glue compressor at a ratio of 3:2, a
  trim of 0.8, then `fx.limiter/1` with a −1.1 dB ceiling.
- **Delivery:** `maac deliver` passed its checks at −15.31 LUFS integrated
  and −1.11 dBTP true peak.
- **Analysis:** the last analysis read −13.6 LUFS integrated, before a trim
  lowered the whole mix for delivery. The sections rise from −19.4 (intro)
  through −16.7, −13.3 and −12.6 to −9.6 (climax), and the coda falls back
  to −18.7.
- **Owner's verdicts** (translated):
  - On version 1: "The harp sounds like a guitar and the violins like a
    keyboard. The arrangement is good, but it doesn't feel classical."
  - On the harp: "c is better, but it still has a guitar-like sharpness,
    something metallic"; then, on version d, "the harp is somewhat harp-like
    now".
  - On the strings, over seven rounds: from "it doesn't sound like a violin
    at all. Why can't synthesis make this? Every sound is a sound wave" to
    "not bad". The [proposal](../design/bowed-strings-proposal.md#why)
    gives each round.

## What happened

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the cue: seven instruments, harmony and voicings for the strings, horns and harp, the theme, an ostinato, rolls, and the hall and master | `maac check` passed after three fixes (F36–F38) |
| Calibration | Seven one-instrument files, each a phrase at velocity 0.7, measured with `maac analyze`. They read from −30.7 to −14.2 LUFS; levels were set so each reads about −20 | F42 |
| Iteration 1 | `analyze --section regions --lead horns:out` | Integrated −10.4 LUFS. The sections spanned 5.5 LU, and the master's +6 dB squashed that arc. The timpani roll was the loudest part of the build, because ten strokes a second with two-second rings pile up. Fixes: the rolls on a node of their own whose level automation shapes each roll (F39); the glue's threshold from −18 to −12 dB; the limiter's gain from +6 to 0 dB |
| Iteration 2 | The same analysis | Integrated −13.6, above the delivery's −14 limit. Sections −19.4, −16.7, −13.3, −12.6, −9.6 and −18.7 LUFS. The limiter's gain could not go to −2 dB (F40), so a trim of 0.8 went before it |
| Delivery 1 | `maac deliver` | 127.6 s, −15.31 LUFS, −1.11 dBTP, checks passed. Analysis took 200 s and delivery 189 s (F43) |
| Verdict 1 | Owner: the harp sounds like a guitar and the violins like a keyboard; the arrangement is good; it does not feel classical | — |
| Round 1 | Harp: a, as before; b, a new harp from `synth.modal/1` playing the same notes; c, the new harp in sweeping arpeggios over two and a half octaves (F44). Strings: a, as before; b, six independent vibratos and drifts, a bow scratch, and a body built in the score from `fx.eq` peaks (F46, F47) | Owner: harp c is better but still metallic; strings b "doesn't sound like a violin at all" and "why can't synthesis make this?" |
| Round 2 | Harp d: the metal came from `stretch` (F45), now 0, with more damping. Strings: a bowed-string prototype outside MaaC, because no graph can hold one (F48): a waveguide with a friction law, a body of about 50 resonances, eight players | Owner: the harp is "somewhat harp-like now", accepted. The solo violin is fine; the ensemble is "a bit ambiguous" |
| Rounds 3–7 | The section, then how notes connect: finger changes, slides, continuous vibrato, the bow change and its timing (F49). The [proposal](../design/bowed-strings-proposal.md#what-made-the-strings-sound-electronic) gives each round | Round 7, "not bad", accepted |
| Proposal | The [bowed strings proposal](../design/bowed-strings-proposal.md): connected notes in the language, a bowed-string node and an orchestral library | The owner accepted every recommendation |

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- The arrangement was approved on first hearing. The owner's complaints were
  all about the sound of two instruments.
- `maac check` named each of the first draft's three errors precisely.
- One `harmony` per section, with voicings, served the strings, the horns,
  the harp and the low strings.
- Tempo ramps in one tempo map gave a build that quickens and a coda that
  slows. Notes, automation and the reverb tail followed them without any
  extra work.
- `synth.modal/1` made a harp the owner accepted: a 0.7 ms pulse into 24
  modes, struck near the middle. The bank was added for the piano and served
  an instrument it was not built for.
- `maac analyze` by region found the problems of the first mix: the master
  squashed the arc, and the timpani roll was the loudest part of the build.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F36 | A part that enters halfway through a harmony reads the harmony from its own start. The trombones, placed at bar 17 with the harmony that begins at bar 13, failed with `E_REFERENCE`. The workaround is `late_pad`, a 32-beat pattern that rests for 16 beats, placed at bar 13 | Open; a convenience |
| F37 | Nodes and instruments share one namespace, so a node `flute` and an instrument `flute` are `E_DUPLICATE_ID`. All seven instruments were renamed | Open; a convenience |
| F38 | The theme pattern carries a pressure swell for the horns. Reused on the flute, it failed with `E_CAPABILITY` until the flute gained a `synth.pressure/1` node | Open; works as specified |
| F39 | A crescendo across a roll's `count` repetitions cannot ramp velocity. The rolls moved to a node of their own, shaped by level automation | Open; a convenience |
| F40 | A limiter's `gain` cannot be negative (`E_RANGE`), so lowering the master's input needed a separate trim | Open; minor |
| F41 | Articulations need separate nodes. Short strings were their own nodes, with an attack of 8–12 ms, so one section was split across nodes and tracks | Open; the [bowed strings proposal](../design/bowed-strings-proposal.md#4-the-instruments-stdorchestra100) keeps it |
| F42 | Instruments built in a song need calibrating by hand. Seven one-instrument files, measured one by one, read from −30.7 to −14.2 LUFS for the same phrase | Open |
| F43 | `maac analyze` took 200 s for 128 s of music with 17 sources, and `maac deliver` 189 s. Every mix iteration waits for both | Open |
| F44 | `synth.pluck/1` is a guitar string. A harp from it reads as a guitar | Fixed in the song with `synth.modal/1`. The harp joins a library in the [bowed strings proposal](../design/bowed-strings-proposal.md#4-the-instruments-stdorchestra100) |
| F45 | `synth.modal/1`'s `stretch` turns a harp into a bell. At 1/5000, a D3's 24th mode sat 94 cents sharp, which the owner heard as "metallic" | Open; the guide should say that a harp needs `stretch = 0` |
| F46 | A violin body cannot be built. An instrument cannot hold `fx.eq/1`, and a shared-stage `synth.svf/1` has no peak gain, so the body had to be a cascade of `fx.eq` peaks in the score after each section. Seven peaks were far too coarse; the accepted body has about 50 resonances | Accepted: the [bowed strings proposal](../design/bowed-strings-proposal.md#3-the-body) |
| F47 | Six detuned saws per voice, with their vibratos and drifts, cost 1.49 billion work units for a 16-beat clip, three times the default profile's limit | Open; the `song` profile covers it |
| F48 | No bowed string can be built from nodes. A bowed string is a loop with a friction law at the bow, and graphs reject feedback | Accepted: the [bowed strings proposal](../design/bowed-strings-proposal.md#2-the-bowed-string-synthbow1) |
| F49 | Every note has its own voice, so a line cannot be played with one bow. The owner heard the connection between notes: a glide between pitches, a bow change during a slide, and a bow change 20 ms late each sounded electronic or unskilled | Accepted: the [bowed strings proposal](../design/bowed-strings-proposal.md#1-connected-notes) |

## What to build next, in order

1. **Bowed strings (F46, F48, F49).** A bowed-string node with its players
   and bodies, connected notes in the language, and a library of orchestral
   strings with the T6 harp. The [proposal](../design/bowed-strings-proposal.md)
   was accepted on 2026-10-20. "Homeward" version 2 is its listening test.

F36–F43, F45 and F47 are conveniences or small fixes. They wait until
another track needs them.
