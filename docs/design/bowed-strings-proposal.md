# Bowed strings: proposal

**Status:** accepted 2026-10-20, when the owner took every recommended
[decision](#decisions). This is item 1 of
[what to build next](../reports/ai-production-trial-t6.md#what-to-build-next-in-order)
in the T6 trial. It covers F46, F48 and F49: a violin body cannot be built
from MaaC's nodes, neither can a bowed string, and a line of notes cannot be
played with one bow.

## Why

T6 was an orchestral film cue, "Homeward". The owner liked the arrangement
but not the strings: "the violins sound like a keyboard; it doesn't feel
classical." The strings were an instrument built in the song from six
detuned saws, two vibratos, rosin noise and a low-pass. MaaC's libraries
have no bowed instrument.

Seven listening rounds followed. The first stayed inside MaaC. The other six
used a Python prototype outside it, because MaaC cannot express what was
needed:

| Round | What changed | Owner |
| --- | --- | --- |
| 1 | Six independent vibratos and slow tuning drifts, a bow scratch at each start, and a body built in the score from seven `fx.eq` peaks and a low-pass | "It doesn't sound like a violin at all. Why can't synthesis make this? Every sound is a sound wave." |
| 2 | Prototype: a bowed-string waveguide, a body of about 50 resonances, eight players with their own vibrato, tuning and timing | "The solo violin is fine; the ensemble is a bit ambiguous." |
| 3 | Fourteen players, narrower vibrato | "The previous one sounds like violins. Now and then, at a legato, it sounds like electronic music; a violin flows, plaintively." |
| 4 | Eight players again. The finger changes the pitch in 4 ms instead of a 10 ms glide; chosen changes slide; the vibrato runs through the line | "At 11 seconds it sounds like electronic music. The romantic one is somewhat better." |
| 5 | A slide into a bow change finishes on the old bow. The bow slows to a stop, rests 30 ms and starts back, and the string is pressed harder for a moment | "It's fine, but there's a very slight delay, like an unskilled player." |
| 6 | The bow change 15 ms earlier, without the rest and a little faster | "The electronic feel is back." |
| 7 | Round 5's bow change, 25 ms earlier | "Not bad." Accepted |

The accepted sound depends on three things that MaaC lacks:

1. **A bowed string (F48).** A bow sets a string vibrating by sticking to it
   and slipping, once per period. A waveguide models this as a delay loop
   with a friction law at the bow. MaaC rejects feedback in a graph, so no
   loop can be built from nodes. `synth.pluck/1` is the precedent: a
   plucked string is a loop too, and it is a native node. The
   [design principles](design-principles.md#give-new-algorithms-explicit-engine-boundaries)
   name this case: "a new string model may justify a processor".
2. **A body (F46).** The wooden body colours the string through about 50
   resonances, with a few strong ones below 600 Hz. In the score, a body was
   a cascade of seven `fx.eq` peaks after each section, because an
   instrument cannot hold `fx.eq` and a shared-stage `synth.svf/1` has no
   peak gain. Seven peaks were far too coarse.
3. **One bow across notes (F49).** In MaaC
   [every note has its own voice](../instruments.md#timing-and-audio-contract):
   its own oscillators, envelopes and filters. A string player keeps the
   string vibrating from note to note. The finger changes the pitch, and the
   bow either continues (a slur) or changes direction. Rounds 3 to 7 were
   about exactly this. The electronic sound came from how notes connect,
   not from the tone of a note.

The [production language review](professional-production-language-review.md#articulation-and-pedal-maps)
held that legato needs "an identified instrument or processor" rather than
a vague core keyword, and "should follow a concrete adapter/library
experiment". T6's rounds are that experiment. This proposal turns the
accepted prototype into MaaC features.

T5 [concluded](../reports/ai-production-trial-t5.md#what-to-build-next-in-order)
that legato is not a missing language feature. That was a synthesizer lead,
and the owner preferred a new note sliding from the previous pitch to one
merged note. A bowed string is different. Its string keeps sounding from
note to note, and the way each change catches the string again is what the
owner heard in rounds 3 to 7. For a bowed string, the connection between
notes is part of the sound.

## What made the strings sound electronic

Each round tested one cause. These are the findings, in the order they were
found:

1. **A saw has no bow.** A bowed string's tone changes with how fast and how
   hard the bow moves. Played louder, it grows brighter and rougher. A saw
   only grows louder. In the prototype, the bow's speed is `0.03 + 0.085 d`
   and its force is `0.5 + 0.35 d`, where `d` runs from 0 (soft) to 1
   (loud). The string stays in its normal motion at any force while the
   speed is at most 0.08. Faster bowing needs more force, or the string
   jumps an octave up.
2. **The body must be dense, and even.** Random bodies passed the
   melody's notes unevenly, by up to 13 dB from note to note. The prototype
   tried thirty and used the four most even, which were within 2–4 dB.
3. **A section is players, not detuned oscillators.** Eight players each
   have their own vibrato, 10–15 cents at 5.1–6.1 Hz. They also have their
   own tuning, off by about 4 cents for the player and 3 for each note, and
   their own timing, about 18 ms on a bow change and 10 ms on a slur. The
   owner preferred them to fourteen players with narrower vibrato (round 3).
4. **The finger changes the pitch; it does not glide.** A 10 ms glide
   between notes is a synthesizer's portamento in miniature (round 3). A
   finger change takes about 4 ms. A slide is a choice made on some changes.
   It is slow to leave and quick to land, and the vibrato narrows during it
   (round 4).
5. **The bow change is where the electronic sound lives.** At every bow
   change the string loses its normal motion for 33–45 ms per player. When
   eight players changed bow in the middle of a slide, together, it sounded
   electronic (round 4, at 11 s). Round 5 fixed this with a bow change that
   finishes the slide first. The bow slows to a stop, rests 30 ms and starts
   back the other way. Round 6 removed the rest and the electronic sound
   came back, so the rest is part of the played sound.
6. **The new bow must speak on the beat.** In round 5 the new bow reached
   its level 20 ms after the beat, which the owner heard as "like an
   unskilled player". Round 7 starts the change 25 ms earlier, so the bow
   speaks 5 ms before the beat. Accepted.

## The proposal at each boundary

Following the [design principles](design-principles.md#evaluate-features-at-the-appropriate-boundary):

- **Core, shared musical meaning:** [connected notes](#1-connected-notes).
  Whether a note continues the one before is a property of the music. Two
  note fields, `slur` and `slide`, carry it.
- **Engine, a new algorithm:** [`synth.bow/1`](#2-the-bowed-string-synthbow1),
  a bowed-string section with [its bodies](#3-the-body) inside it.
- **Library, reusable instruments:** [`std/orchestra/1.0.0`](#4-the-instruments-stdorchestra100),
  with violins, violas, cellos, basses and the T6 concert harp.
- **Tools:** the producer guide, and checks in `maac analyze` for connected
  notes.

## 1. Connected notes

### Joining

A note **joins** the note before it when:

1. both are on the same track,
2. it starts on the frame where the earlier note's gate ends,
3. both have the same chord member index. A note that is not a chord member
   counts as member 0, and so does a chord that plays one pitch through
   `pick`, `top` or `bottom`.
4. their destination plays connected notes.

A joined note continues the earlier note's voice instead of starting a new
one. A note that starts later than the earlier one ends, even by one frame,
starts a new voice, as today. So does a note that overlaps the earlier
one. A rest therefore lifts the bow, and notes that touch are played
connected. Chords join member by member. A pad of three-note chords is
three connected lines, which is how a divided viola section plays it.

A destination plays connected notes when its voice graph contains a
`synth.bow/1` node. It is derived from the program, as `synth.timbre/1`
opts an instrument into timbre. No other instrument joins notes, so every
existing song renders bit-identically.

### Two note fields

```maac
note n3 { at = 4q; dur = 3/2q; pitch = Bb5; velocity = 0.68; }
note n4 { at = 11/2q; dur = 1/2q; pitch = A5; velocity = 0.6; slur = true; }
note n5 { at = 6q; dur = 1q; pitch = G5; velocity = 0.6; slide = 120ms; }
```

- `slur` is a boolean, default false. A slurred note is played in the same
  bow as the note it joins: only the finger moves. Without it, a joined note
  changes bow.
- `slide` is a duration from 0 s (the default) to 250 ms. The pitch slides
  from the earlier note over about that time instead of changing at once.

Both apply only to a joined note. On a note that joins nothing, either field
is `E_JOIN`. The error names the nearest earlier note with the same member
index on the track and where it ends. On a destination that does not play
connected notes, either field is `E_CAPABILITY`, as for an unsupported
expression. A chord leaf's fields apply to every member.

### In the plan

A joined note event gains `join`, the address of the note it continues, and
`slur` and `slide_seconds` when set. The fields are optional, as the
expression fields are, and absent fields keep today's JSON. Validation
checks that the joined note is on the same target, that its `on_frame`
equals the other note's `off_frame`, and that each note is joined at most
once.

### The voice

At a joined note's onset the voice is not released. It takes the joined
note's address, base pitch, velocity and expressions, and every node keeps
its state. Envelopes do not restart, and oscillators, filters and the string
continue. Expressions are evaluated on each note's own gate, as today.

The velocity that scales the voice's output moves to the new note's
velocity over 100 ms rather than stepping, so a louder note does not click.
`synth.velocity/1` emits the same moving value. `synth.key/1` changes at the
onset.

### Knowing the next note

A player reads ahead. The bow change starts 65 ms before the beat, and a
slide into a bow change finishes before the change begins. A voice that
plays connected notes therefore learns each joined note 500 ms before its
onset: its pitch, velocity, `slur` and `slide`. This is part of the
contract, not hidden compensation. The output at frame `t` depends only on
notes that start by `t` + 500 ms, and a test checks this by cutting a line
after `t` + 500 ms. The fields' limits keep the reading within 500 ms:
`slide` at most 250 ms, varied by up to 20 % per player; the bow change's
65 ms; and timing spread at most 3 × 40 ms. Voices still start at their
first note's onset.

Offline rendering is unaffected. Live playback through such an instrument
would need 500 ms of latency. Sampled string libraries ask something
similar of their users, who shift legato tracks earlier by up to a few
hundred milliseconds.

## 2. The bowed string: `synth.bow/1`

A voice-only source with no inputs. It plays the voice's line on a section
of players. Each player has its own string, bow and body.

| Field | Meaning | Default; range |
| --- | --- | --- |
| `config.players` | Players in the section | 1; 1…16 |
| `config.body` | Body family: `violin`, `viola`, `cello`, `bass` or `none` | `violin` |
| `config.channels` | 1, or 2 to spread the players across the stereo field | 1 |
| `config.seed` | Nonzero unsigned 32-bit seed for the players' differences | 1 |
| `ratio` | Multiplier on the voice frequency | 1; 1/8…8 |
| `dynamics` | How fast and how hard the bow moves, soft to loud | 1/2; 0…1 |
| `position` | Bow distance from the bridge, as a fraction of the string | 1/8; 1/16…1/5 |
| `vibrato` | Vibrato depth, in cents | 25/2; 0…50 |
| `rate` | Vibrato rate | 28/5 Hz; 3…8 Hz |
| `attack` | Time for a stroke after a rest to reach full speed | 80 ms; 5…500 ms |
| `timing` | Spread of the players' bow changes | 18 ms; 0…40 ms |
| `spread` | Scale on the players' differences in tuning, vibrato, slides and level | 1; 0…1 |
| `width` | Stereo spread of the players, with two channels | 7/10; 0…2 |
| `pan` | Centre of the section, with two channels | 0; −1…1 |
| `level` | Output gain | 1; 0…16 |

`attack` is captured at the onset of a stroke after a rest. `timing` is
captured at each onset. The other parameters are sampled every frame. With
`spread = 0` and `timing = 0` the players play the same pitches at the same
times, and only their bodies and bow noise differ. The tests use this.

### The string

Each player's string is a loop of two delay lines, split at the bow. The bow
sits at `position` from the bridge. The loop's length is set so that the
string plays at its pitch: it is shortened by the bridge filter's phase delay
at that frequency. Delays use linear interpolation. Each frame:

```text
v_s = r_bridge + r_nut                       string velocity under the bow
Δ   = v_bow − v_s
k   = min(1, (|Δ (5 − 4 F)| + 3/4)^−4)       friction: stick when Δ is small
new = Δ k                                    added to both outgoing waves
r_nut    = − (wave arriving at the nut)
r_bridge = − lp(wave arriving at the bridge)
lp: y ← 0.95 (1 − 0.658) x + 0.658 y         the bridge's loss
```

Here `F` is the bow force from 0 to 1. The bow's speed carries a little
noise, `v_bow × (1 + 0.4 n)`, where `n` is uniform noise through a one-pole
smoother with pole 0.995. The output is the wave arriving at the bridge,
which drives the body. These are the prototype's values. Its 1.6-sample
correction is replaced by the exact phase delay at the played frequency.

### The bow

`dynamics` sets the bow's speed, `0.03 + 0.085 d`, and its force,
`0.5 + 0.35 d`. Both follow `d` smoothed over 100 ms. The direction of
travel alternates at each bow change. A change centred at time `c`, 25 ms
before the joined note's onset, goes through four parts:

1. Over 25 ms the speed falls to zero through a quarter cosine.
2. The bow rests for 30 ms.
3. Over 25 ms the speed rises in the other direction through a quarter sine.
4. Across the change the force rises by up to 0.25, as a bell 40 ms wide
   centred at `c`. This is the press that makes the string catch again. The
   force is capped at 0.95.

The change runs from 65 ms before the onset to 15 ms after it. A stroke
after a rest starts at the note's onset and reaches full speed through a
quarter sine over `attack`, with the same press 20 ms in. When the line
ends, the bow lifts: its speed falls linearly to zero over 250 ms from the
note-off. The string then rings out through its loss and the body.

### The player

- **Finger changes.** At a joined note without `slide`, the pitch moves in
  4 ms, centred on the player's onset. The vibrato drops to 60 % of its
  depth and returns over 180 ms.
- **Slides.** Each player's slide lasts `slide`, longer or shorter by a
  random amount of up to 20 % × `spread`. A
  slurred slide runs from 65 % of it before the onset to 35 % after. A slide
  into a bow change ends where the change begins, 65 ms before the onset.
  The pitch moves in cents as `u²` over the slide: slow to leave, quick to
  land. During the slide the vibrato is at most 20 % and the bow lightens by
  up to 15 %. Afterwards the vibrato returns over 300 ms.
- **Vibrato.** The vibrato is continuous through the line. It is not
  restarted on each note. Each player's depth is `vibrato` × a random
  0.8–1.2 and their rate is `rate` ± up to 0.5 Hz, both scaled by `spread`.
  It grows from 30 % to full over the line's first 250 ms. On a note held
  1.2 s or longer it widens by up to 25 % towards the note's end. Depth
  changes are smoothed over 30 ms.
- **Tuning.** Each player is off by a random amount, with a standard
  deviation of 4 cents, and each note by another 3 cents. Two slow
  drifts are added: 3 cents at 0.08–0.2 Hz and 2 cents at 0.2–0.5 Hz. All are
  scaled by `spread`.
- **Timing.** Each player's onset at a bow change differs by a random
  amount with standard deviation `timing`, cut off at 3 × `timing`. On a
  slur it is 5/9 of that. A stroke after a rest only ever comes late.
- **Level.** Each player is off by a random amount with a standard deviation
  of 1.5 dB × `spread`.

All random values come from `config.seed` through a fixed, documented
generator. Each player and each note draws in a fixed order, so the same
source renders identically. With two channels, the players sit evenly
across `pan` ± `width`/2, equal-power, clamped to the field.

## 3. The body

The body is inside `synth.bow/1`, one for each player. The accepted
prototype gave its eight violins four different bodies, two players to
each. It was rendered again with one body for the whole section, and the
section sounded different: its lowest band was 5.6 dB weaker and one middle
band 2.7 dB stronger. See [decision 4](#decisions).

A body is a bank of resonators: five signature modes and about 45 others.

- **Signature modes**, for a violin: 275 Hz (Q 22, 0 dB), 405 Hz (Q 30,
  −4 dB), 470 Hz (Q 40, +2 dB), 540 Hz (Q 40, +1 dB) and 590 Hz (Q 35,
  −3 dB).
- **Dense modes**, from 620 Hz to 10 kHz, each a random 3–9 % above the
  last. Each has a random Q of 15–35 and a level that follows the bridge
  hill:
  - 0 dB from 250 to 650 Hz, falling 12 dB per octave below 250 Hz;
  - −6 dB from 650 to 1600 Hz;
  - a rise to +3 dB between 1.6 and 4 kHz;
  - a fall of 12 dB per octave above 4 kHz.

  Each mode's level also varies by −4 to +1 dB.

A family scales every frequency: violin 1, viola 0.8, cello 0.42 and bass
0.3. Each mode also varies by a random 3 %. Signs are random, and the
bank's impulse response is normalized to unit energy. Each mode is a
decaying sinusoid, as in `synth.modal/1`, but at a fixed frequency rather
than a multiple of the note.

Random bodies pass some notes much louder than others. The node therefore
does not draw bodies at render time. Each family has a fixed table of 16
bodies: the most even of 120 draws, the prototype's 4 of 30 at a larger
scale. A body's evenness is the spread, across the family's range semitone
by semitone, of the energy it passes from a note's harmonics, each weighted
by `1/k²`. Player `k` plays body `(seed + k) mod 16`. The table is part of
the node's version.

## 4. The instruments: `std/orchestra/1.0.0`

A new library with five instruments:

| Export | Players | Body | Range | Notes |
| --- | --- | --- | --- | --- |
| `violins` | 8 | violin | G3–A7 | Vibrato 12.5 cents at 5.6 Hz, timing 18 ms |
| `violas` | 3 per line | viola | C3–E6 | Vibrato 11 cents at 5.45 Hz, timing 30 ms |
| `cellos` | 5 | cello | C2–A5 | Vibrato 10 cents at 5.2 Hz, timing 30 ms |
| `basses` | 3 | bass | E1–G4 | Vibrato 6.5 cents at 5 Hz, timing 30 ms |
| `concert_harp` | — | — | C1–G7 | T6's accepted harp, a pulse into `synth.modal/1` |

Each string instrument maps velocity and pressure to `dynamics`. It exposes
`level`, `attack`, `vibrato`, `rate`, `position`, `timing` and `low_cut` as
controls, and places the section in the stereo field. The section's size is
fixed by the export. Its vibrato, timing and spread are close to the
prototype's for that section. Short strokes after a rest use the same
instrument with a shorter `attack`, on a node of their own as today, so
F41 stays open.

The hall stays in the score, through `fx.reverb`.

## Effect on "Homeward"

The cue imports `std/orchestra` and drops its in-song strings and harp. The
violins' theme lines join where the notes touch, with slurs and slides
written as a player would bow them. The violas' close chords become three
connected lines through member indices. The pads are bowed two bars to a
stroke, with the chord change slurred. The fast arpeggio's sixteenths
touch, so the violins change bow on every note. The basses' short pulses
leave gaps, so each is a stroke of its own, with a short `attack`. Version
2 of the cue is the trial's delivery and the listening test of this
proposal.

## Plan

1. **`synth.bow/1` without connected notes.** The string, bow, players,
   vibrato and bodies, with reference vectors and benchmarks for the work
   charge. Each note is still its own stroke.
2. **Connected notes.** `slur` and `slide` in the language, joining in the
   compiler, the plan fields, the voice that continues, and reading ahead.
   Diagnostics, conformance vectors, and the spec text in §8.
3. **Match the prototype.** Render the round 7 excerpt through MaaC and
   compare it with the prototype: when each bow change speaks, how long the
   string leaves its normal motion, band levels and clicks.
4. **`std/orchestra/1.0.0`,** frozen after step 5.
5. **"Homeward" version 2,** with one listening round: the excerpt from
   MaaC against the accepted prototype, then the full cue.
6. **Documentation:** the instruments guide, the producer guide, the
   capability matrix, the CHANGELOG and the plan's AI-production row.

## Acceptance

- **Prototype match.** The round 7 excerpt from MaaC has bow changes that
  speak within 5 ms of the prototype's −5 ms. Each player leaves normal
  motion for at most 45 ms per change. Band levels are within 2 dB of the
  prototype's, and no 5 ms frame above 6 kHz rises 8 dB over its local
  median.
- **In tune.** With `spread = 0`, `timing = 0` and `vibrato = 0`, every
  semitone from G3 to A7 plays within 3 cents of its pitch at
  `dynamics` 0.2, 0.5 and 0.9.
- **Stable.** Across the whole range of `dynamics` and `position`, the
  string stays in normal motion after the first 100 ms of a stroke on every
  semitone of each family's range.
- **Owner.** The excerpt from MaaC is heard as the same as, or better than,
  the prototype's. The strings of "Homeward" version 2 are heard as
  violins, not electronic.
- **Unchanged songs.** Every existing example, fixture and trial song
  renders bit-identically.
- **Connected notes.** Tests cover each joining rule and each `E_JOIN`
  reason. A cut-off test shows that the output at `t` depends only on notes
  starting by `t` + 500 ms. Repeated renders are bit-identical.
- **Cost.** "Homeward" version 2 builds within the `song` profile, and the
  report records its work units and render time.

## Cost and risk

- **Connected notes change the voice model.** Until now one note has meant
  one voice. Joining touches the compiler, the plan, the renderer's voice
  handling, voice-capacity accounting and analysis. The change is limited to
  instruments that contain `synth.bow/1`, and every other song stays
  identical. Still, it is the largest change to the renderer since per-note
  expressions.
- **Reading ahead is new.** Rendering is offline, so knowing 500 ms ahead is
  cheap. A future live mode would need that much latency for these
  instruments.
- **Work.** Each player costs a string and about 50 resonators. "Homeward"
  has 25 string players in its widest passage, and the fast arpeggio adds
  overlapping voices. The work charge is fixed by a benchmark, and the
  acceptance records the total. In the prototype, the 25 players of the
  round 7 excerpt took 6.3 s of CPU for 16 s of audio.
- **The model can squeal.** A waveguide with friction can lose its normal
  motion and jump an octave at high speed and low force. The prototype
  measured where it holds, and the `dynamics` mapping stays inside that.
  The stability test checks it across the whole range of `dynamics` and
  `position`. If a bow position fails, the range narrows before the freeze.
- **One way of playing.** The model plays a romantic legato and détaché.
  Pizzicato, tremolo, spiccato, harmonics and mutes are not covered.
- **The owner's verdict was "not bad".** The owner accepted it, but was not
  enthusiastic. The MaaC version may need more rounds. The prototype stays
  available for them.

## Not proposed

- **Connected notes on other instruments.** The rule is general, but only
  `synth.bow/1` advertises it here. Joining for winds, brass and synthesizer
  leads can follow, each with its own model.
- **Other articulations:** pizzicato, tremolo, spiccato, col legno,
  harmonics and mutes.
- **Two notes at once on one line.** Double stops are separate voices that
  overlap.
- **A separate body node.** A guitar or piano body could reuse the bank
  later. Here it stays inside the bow, one per player.
- **Phrasing inside the node.** Swells stay in the score as pressure curves,
  as in the prototype, which swelled long notes with a curve.
- **Sampled strings,** which need recordings and a licence position.

## Decisions

The owner accepted every recommendation on 2026-10-20 ("go with the
recommendations"). Asked first whether this is a language change or an
instrument library, the owner was told that it is mostly an engine node and
a library, and that the language gains two note fields and the joining
rule.

1. **Approach:** a synthesized bowed string, built into MaaC as a new node
   (recommended). The alternatives are samples, which need recordings and a
   licence, or improving the saw strings, which seven rounds showed cannot
   reach a violin.
2. **Connected notes:** automatic for bowed instruments. Notes that touch are
   played connected and a rest lifts the bow (recommended). The
   alternative is to mark every connected note by hand, which is more
   writing and easier to get wrong.
3. **The section:** players inside the node, `players = 8`, each with their
   own tuning, vibrato and timing (recommended, the accepted sound). The
   alternative is one player per node, with the section built in the score
   from eight nodes and eight tracks. That cannot vary timing note by note.
4. **The body:** one per player, inside the node (recommended, the
   accepted sound). One body per section would be eight times cheaper, but
   it sounded different, and choosing it would need a listening round.
5. **Reading ahead:** the instrument knows the next notes 500 ms ahead, so
   bow changes land on the beat and slides finish on the old bow
   (recommended). The alternative is strictly causal playing, where every
   bowed note is written early by hand, as sample-library users do.
6. **Library:** a new `std/orchestra/1.0.0` with violins, violas, cellos,
   basses and the concert harp, leaving `std/studio` unchanged
   (recommended).
7. **Listening:** one round before the freeze, with the MaaC excerpt against
   the prototype and then the full "Homeward" version 2 (recommended).
