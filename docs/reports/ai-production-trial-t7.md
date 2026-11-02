# T7: a sixth AI production, a jazz quartet

**Date:** 2026-10-23 to 2026-11-02. **Producer:** Claude, an AI model,
working in this repository. **Listeners:** the project owner, and once a
second listener, a jazz fan. **Result:** "After the Rain", version 4
([`examples/rain/rain.maac`](../../examples/rain/rain.maac)). The owner
closed the trial with the arrangement approved and the saxophone's sound
still open.

T2 to T6 covered lo-fi hip-hop, folk-pop, pop, dance music and an orchestra.
T7 tested jazz, the owner's choice: a small band that swings, a horn that
leads, and walking bass and comping that follow the chords. No MaaC library
has a saxophone, a double bass or a ride cymbal, so all three were built in
the song. Friction numbers continue from T6.

## Brief and rules

- **Brief:** jazz, the owner's choice. The AI chose a medium swing for a
  quartet, "After the Rain": tenor saxophone, piano, double bass and drums,
  an AABA tune of 32 bars in B-flat at 160 bpm, played, then a chorus of
  saxophone solo, then the tune again from its bridge, and a tag.
- **Tools:** `maac check`, `maac analyze` (profile `song`,
  `--section regions`, `--lead sax:out`), `maac compile` and `maac render`
  for clips, and `maac deliver`. The source was not written directly: a
  Python script of the AI's own wrote every note from compact strings, a
  bar at a time (F53), around instruments and a mix written by hand. Two
  pieces of work were outside MaaC: a reed prototype (round 3), and a
  measurement of the saxophone against single notes of a real tenor
  saxophone from
  [UNSW Music Acoustics](https://newt.phys.unsw.edu.au/music/saxophone/tenor/B3.html),
  used for analysis only.
- **Judgment:** the owner listened on a phone. After the first version,
  each round was blind: pairs of 25-second clips from the start of the
  song, at −18 LUFS, one change per pair, the letters chosen by coin and
  revealed after the verdict.

## Outcome

- **Form:** 88 bars, about 133 s plus a 4 s tail: intro (bars 1–4), the
  tune (5–36), the solo (37–68), the tune from its bridge (69–84) and the
  tag (85–88), each a region. The tag slows from 160 to 112 bpm.
- **Parts:** 2069 notes and 615 hits in 1745 lines.
  - Built in the song: a tenor saxophone (63 lines), a saw and a sine
    through a key-tracked low-pass, a drive and two formants, with a lip dip,
    a tongue and breath noise; a double bass played pizzicato (47), a pulse
    into 20 string modes in `synth.modal/1`; a ride cymbal (74), a stick
    tick, six `synth.modal/1` banks of inharmonic modes from 2 kHz up and a
    wash of noise.
  - From `std/studio/1.2.0`: the piano, with rootless voicings over five
    `harmony` declarations, and the drum kit.
  - The saxophone's 457 notes each carry a velocity and a breath, a
    pressure curve, drawn from seeded random rules; 457 of the song's 459
    curves are those breaths.
- **Mix and master:** five nodes into one sum, a 1.4 s `fx.reverb/1` room
  returned at 0.22, a glue compressor at 3:2, a trim of 1.4, then
  `fx.limiter/1` with a −1.1 dB ceiling.
- **Analysis of version 4:** sections at −16.1 (intro), −14.6, −14.6, −14.6
  and −14.8 LUFS; the saxophone's lead margin +6.4 dB in the tune, +4.0 in
  the solo, +6.3 in the tune's return and +5.9 in the tag.
- **Delivery:** `maac deliver` passed at −14.67 LUFS and −1.10 dBTP. The plan
  charges 11.3 billion units of work, within the `song` profile. For
  version 1, analysis took 65 s and delivery 60 s.
- **Owner's last word** (translated): "Let's leave it here for now. The
  arrangement itself is fine."

## What happened

Verdicts are translated.

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote the tune, a solo chorus, walking bass, comping, the drums and three instruments | `maac check` errors: F55, F56, and F37 twice |
| Calibration | Three one-instrument files, each a four-bar phrase, set to −20 LUFS | F42 |
| Version 1 | `analyze --section regions --lead sax:out`, then delivery | Lead margin +5.5 to +6.9 dB; −15.52 LUFS, −1.10 dBTP |
| Verdict 1 | Owner: "The ride cymbal is the funniest, and the saxophone is so frivolous, like game electronics. There's some swing" | The ride was six inharmonic square waves, a drum machine's cymbal |
| A bug | The AI's script read the "/" of every fraction as its scoop mark: 383 of the 457 saxophone notes slid up 120 cents over 90 ms | F53 |
| Round 1 | Saxophone: the bug fixed, against version 1 as heard. Ride: a new ride, a stick tick, two pitched modal pings and two noise bands | Owner: "b is better for both", the scoops of the bug and the new ride, "but the saxophone has no groove, and the ride spreads too much. It should ping too" |
| Round 2 | Saxophone: jazz phrasing, on-beat eighths ghosted, off-beats accented, phrase ends short, the part laid back 18 ms (F58). Ride: the ping 6 dB louder, the wash shorter | The phrasing won; the louder ping lost |
| Round 3 | Saxophone: a reed waveguide prototype outside MaaC, after STK's Saxofony, played from the plan's saxophone events. Ride d: the pings replaced by six banks from 2.1 kHz up, about 30 modes and no one pitch | The reed "just felt like a vuvuzela" and lost. Ride d won. F57 |
| Round 4 | The tune an octave lower, into the tenor's middle; breath noise that pulses with the reed | "The low one is somewhat better"; the breath, "can't tell them apart"; ride d, "good" |
| Version 2 | Version 1 with the scoops, the phrasing, the lower octave and ride d | The lower saxophone led by only +0.6 to +3.3 dB (F51 again); 3.5 dB louder, +4.0 to +7.1. −14.77 LUFS |
| Verdict 2 | The second listener: "like a drunk violin" | Two readings: the scoops on almost every note ("drunk"), and a saw with a soft attack ("violin") |
| Round 5 | Slide: smaller, faster scoops. Attack: a tongue, a 6 ms attack, a 20 ms "t" of noise and a tone that opens from dark | The big scoops stayed; the tongue won. Is it still a drunk violin? "No, it seems better now." F59 |
| Version 3 | Version 2 with the tongue | −14.72 LUFS |
| Verdict 3 | Owner, comparing with real saxophone recordings: "it clearly lacks a tight, dense core, and spreads out with a hollow 'boo'" | — |
| Measurement | Twelve notes of version 3's saxophone against the UNSW tenor's, as harmonic power in third-octave bands | 2.5–6.4 kHz 4 to 11 dB too weak; the low notes' second harmonic 11 to 14 dB too strong; the fundamental 8 to 22 dB too weak, from a sine that had cancelled it since version 1 (F63) |
| Round 6 | A tone fitted to the recordings by a search over four settings, in the band and alone; at the owner's request, again an octave higher | No verdict: the owner turned to how uniform the saxophone was |
| Uniformity | Owner: "Maybe it's because it's too uniform. A wind instrument should have something irregular, but the way it gets louder and softer..." | Every eighth at 0.46 on the beat and 0.70 off it, nine velocities in the part; one 250 ms sag on every note; repeats played verbatim (F60–F62) |
| Round 7 | Accent: velocities from an arc over each phrase, with chance accents and ghosts, seeded. Breath: each note its own pressure curve, louder and brighter with more | "b is better for both, but the breath feels a little artificial": both adopted |
| Round 8 | Flow: one breath for each phrase, which its notes follow. Bright: pressure brightens the tone by 600 Hz instead of 1600 | "b is better for both. Yes, less artificial": both adopted |
| Version 4 | Version 3 with the accents and the flowing breath | −14.67 LUFS, −1.10 dBTP |
| Round 9 | Round 6's fitted tone again, on version 4 | No verdict. The owner closed the trial |

Two lessons for the next trial:

- **A listener's image names an impression, not a setting.** "Drunk"
  suggested the scoops; the owner kept them, and the tongue answered "drunk
  violin". Blind pairs that each change one thing were what told the two
  apart.
- **The second clip may be favoured.** In the pairs the owner decided, b
  won 11 of 13, whether b was new or old: in round 3, b was the old
  saxophone, and in round 1 the bug. The letters were chosen by coin, but a
  later trial should send one pair twice with the letters swapped.

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- The arrangement was never in question. Every complaint over nine rounds
  was about the sound of the saxophone or the ride, and the owner closed
  with "the arrangement itself is fine".
- Two grooves swung on first hearing: eighths at 3:2 for the saxophone,
  the piano and the snare, and the triplet for the ride and the hi-hat.
- One `harmony` per section and one set of `voicings` gave the piano its
  rootless voicings in every chorus.
- A ride built in the song from six `synth.modal/1` banks and noise was
  accepted in round 4, after the drum machine's cymbal had been "the
  funniest" part of version 1.
- The check that T6 added to the [producer guide](../ai-producer-guide.md)
  for F51 worked: `--lead sax:out` showed the lower saxophone buried before
  the owner heard it.
- `synth.pressure/1` with a curve on each note gave the saxophone a breath
  that the owner heard as less artificial once it followed the phrase.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F52 | A saxophone's line cannot be slurred. Only an instrument with a `synth.bow/1` plays [connected notes](../../MaaC-1-Specification.md#83-connected-notes); every other note starts its own voice, so each eighth is attacked anew | Open |
| F53 | A line cannot be written as a succession of notes: each note needs its own `at`. The AI wrote a script to place all 2069 notes and 615 hits, and the script's own parsing bug put a 120-cent scoop on 383 of 457 saxophone notes in version 1 | Open |
| F54 | Chord names are labels. A walking bass cannot ask for the chord's root or third, so every line was written out. A comping hit on the last eighth of a bar, anticipating the next chord, reads the current one, so the comping avoided anticipations | Open |
| F55 | `maac instruments --library std/studio/1.2.0 drums` lists `hat_brightness: default 14000` with no unit; `14000` is `E_UNIT`, and `14000Hz` is needed | Open; small |
| F56 | Each stroke on a ride is a new voice, so a ride in time is many cymbals at once. Twelve voices were `E_VOICE_LIMIT`; the song gives the ride 32 | Open |
| F57 | A 19.5-second mono take of 3.7 MB cannot be compiled: the plan embeds it as a JSON array of numbers, over the 4 MiB limit (`E_RESOURCE_LIMIT`). `build --disk-media` works, but `build` has no render range, so each 25-second clip rendered the whole song | Open |
| F58 | A part cannot be laid back. The 18 ms lay-back is an `onset_offset` and a `release_offset` on each of the 457 saxophone notes; a `groove` has only a grid and a ratio | Open |
| F59 | A filter's cutoff can be modulated below zero on a single note. A dark start of −1500 Hz was `E_NONFINITE` on a soft low note; finding the floor meant adding the base cutoff, key tracking and every modulation's depth by hand | Open |
| F60 | Nothing varies a performance. An instrument cannot differ from one note to the next, since `synth.noise/1` restarts from its seed in every voice and no source gives a note a random value, and a score has no humanize. What the owner heard as "too uniform" was rules: two velocities for the eighths and one envelope for every note. All the variation in version 4 was drawn by the AI's script | Open; first below |
| F61 | A curve cannot be written inline. Each note's breath is a named curve: 457 of them, a quarter of the song | Open |
| F62 | A placed pattern repeats verbatim. To play the tune's second and third A differently, three patterns were copies | Open |
| F63 | The saxophone's sine, meant to add to the saw's fundamental, cancelled most of it from version 1 on. The [instruments reference](../instruments.md) gives the saw's harmonics as `-2/(pi*k)` and the sine's as `+1`, so at the same phase they oppose; the AI missed it. Only the measurement against a recording found it. Version 4 still has it; round 9's fitted tone removes it | Open; works as specified. The producer guide could say that a sine reinforces a saw at phase 0.5 |

F37, F42 and F51 recurred. F37: `E_DUPLICATE_ID` twice, a placement and a
node both named `ride`, and a placement and a pattern both named
`kick_end`. F42: three instruments calibrated by hand. F51: matched alone,
the lower saxophone was buried in the band (see the table above).

## What to build next, in order

The owner decides; this is the AI's recommendation.

1. **Performance variation (F60, F61, F62).** The owner named it as the
   fault, and both rounds that addressed it won. Today it needs a script
   outside MaaC and 457 named curves. A proposal should weigh a per-note
   random source for instruments, a seeded humanize on placements that
   varies each repetition, and inline curves.
2. **Writing a line (F53, F52, F58).** Notes written in succession, connected
   notes for instruments without a bow, so that a saxophone can slur, and a
   lay-back for a whole part. Without them, the AI could not write the song
   by hand.
3. **The saxophone's tone (F63).** Round 6's fitted tone, which removes the
   cancelling sine and adds the buzz of a real tenor, is ready and has had
   no verdict. A woodwind library should wait for one.

F54–F57 and F59 are conveniences. They wait until another track needs them.
