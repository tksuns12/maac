# AI producer guide

How an AI can produce a finished track with MaaC: write source, render it,
perceive it through measurements and images, revise, master and deliver.
Every step below was used in the [T2](reports/ai-production-trial.md),
[T3](reports/ai-production-trial-t3.md) or [T4](reports/ai-production-trial-t4.md)
production trial, and the pitfalls are ones those trials hit.

You cannot hear your own render. `maac analyze` is your measurement of what
you would otherwise hear, and the person you work for is the final judge of
anything a number cannot settle: timbre, annoyance and feel. Ask them to
listen before you call a track finished.

## 1. Plan before writing

- **Instruments.** Start from `std/studio/1.1.0`
  (`maac instruments --library std/studio/1.1.0`): its electric piano,
  acoustic piano, electric guitar, bass, pad and `drums` kit change timbre
  with velocity, and their levels share one calibrated scale. Use the
  `std/acoustic/1.0.0` guitars for picked acoustic parts; for strums, see
  the pitfalls below. For a guitar lead, set the electric guitar's
  `pickup = 0.3` and `tone = 3200Hz`, and automate its `drive` alone: the
  level holds. Leave its `vibrato` control at 0 and give held notes a
  `vibrato` of their own (see section 2).
  std/basic covers other roles, such as organ, strings, flute, bell and
  leads, with older synthesized timbres that ignore velocity except for
  loudness. Its guitars sound like a bass and its `finger_bass` has no
  pluck, so play those parts on std/acoustic and on std/studio's `bass`.
- **Form.** Fix the tempo, the meter and the form in bars. Declare each
  section as a `region`, so the analysis reports per section by name.
- **Groove grid.** Choose one grid, straight or swung, for every part. For
  swing, declare one `groove`, such as `groove lazy { grid = 1/2q; ratio = 2/3; }`,
  write every part on straight positions, and give each placement
  `groove = &lazy`. The groove then moves an off-beat eighth at `x + 1/2q` to
  `x + 2/3q` and a sixteenth at `x + 3/4q` to `x + 5/6q`. At 90 bpm swung and
  straight eighths are 111 ms apart, which is plainly audible; mixing them was
  the trial's worst mistake.

## 2. Write compactly

- Write each chord as one `chord` leaf:
  `chord down { at = 0q; dur = 7/5q; pitches = [A3, C4, E4, G4]; velocity = [0.5, 0.45, 0.45, 0.42]; }`.
  Give `velocity` a list to voice it.
- Strum with `strum = down` or `strum = up`, not with offset lists. The
  strings are 8 ms apart unless `stagger` says otherwise, and `tilt` is the
  velocity change from the first string struck to the last:
  `strum = down; velocity = 0.78; tilt = -0.12;`.
- Name each guitar shape once as a `voicing`, such as
  `voicing d { pitches = [D3, A3, D4, F#4]; }`, and write the strum rhythm
  once. Chords without `pitches` play the voicing their `use` or `place`
  names: `use b2 { pattern = &strum_bar; at = 4q; voicing = &d; }`. `top = 4`
  keeps the upper four strings, as an upstroke catches them.
- Write each section's chords once, as a `harmony`, and each part's voicing
  for every chord once, as its `voicings`:
  `harmony verse_chords { length = 32q; chords = [(0q, "D"), (4q, "A"), (8q, "Bm"), (12q, "G")]; }`
  and `voicings piano_voicings { chords = [("D", &pno_d), ("A", &pno_a), ("Bm", &pno_bm), ("G", &pno_g)]; }`.
  Then place a one-bar rhythm of chords without pitches with `count`, the
  harmony and the part's voicings:
  `place piano_verse { pattern = &pno_pulse; track = &piano_notes; at = 16q; count = 8; harmony = &verse_chords; voicings = &piano_voicings; }`.
  Each chord plays the voicing for the chord sounding at its own position,
  and every part reads the same harmony. A rhythm that changes within a
  section is a short pattern of `use` lines. Name these objects by role,
  such as `verse_chords` and `piano_voicings`: a harmony called `bridge`
  clashes with a region of that name, and voicings called `piano` with the
  node.
- Give a melody vibrato and bends per note. Declare the vibrato once and
  name it from each held note's pitch expression, after a bend if the note
  has one:
  `vibrato guitar { rate = 11/2Hz; depth = 38ct; delay = 300ms; rise = 450ms; shape = up; }`,
  then `expression vib { kind = pitch; vibrato = &guitar; }`. `shape = up`
  only rises from the pitch, as a guitarist's does; voices and strings use
  the default `centered`. A bend is a seconds-clock curve that ends by the
  vibrato's delay: `points = [(0ms, -200ct, step), (40ms, -200ct, linear), (170ms, 0ct, step)]`
  picks a whole step low and pushes up. See
  [vibrato](pitch-expression.md#vibrato).
- Let the bass follow the harmony too. Give it a voicing per chord of the
  root and its octave, such as `voicing bs_a { pitches = [A1, A2]; }`, and
  write its rhythm as chords with `bottom = 1` for the root and `top = 1`
  for the octave. The register stays yours, and no transposition needs
  working out.
- Reuse a line at other pitches with `transpose` on `use`, in cents:
  `use d_bar { pattern = &g_bar; at = 4q; transpose = 700ct; }` plays a G
  pattern as D. Mind the range: a transposed shape keeps its spread, so a G
  shape from G2 moved down a fourth plays D2, below a guitar's low E (E2).
  "Morning Ferry"'s picking does this in its D and C bars; give a picked
  guitar a pattern per shape instead.
- Play drums from one kit node: import `std/studio/1.1.0`, add
  `node drums { instrument = &studio.drums; }`, and write `hit` leaves such as
  `hit k1 { at = 0q; key = "kick"; velocity = 0.85; }`. Keys are `kick`,
  `snare`, `clap`, `hat`, `open_hat`, `low_tom`, `high_tom` and `crash`; set
  each drum with `kick_level`, `hat_pan` and so on. A kit has one output, so
  a drum that needs its own send or effect stays a separate instrument node.
- Connect everything to an explicit stereo `core.sum/1` bus.
- Name nodes by their role, such as `strum` or `picking`, not after an
  instrument library. Imports, nodes and every other top-level object share
  one ID namespace, so `node acoustic` next to `import acoustic` is
  `E_DUPLICATE_ID`.
- Vary velocity. On std/studio a harder note is brighter as well as louder,
  so accents and ghost notes change the sound; a part at one velocity sounds
  like a machine.
- Mind the level scales. Every std/studio default puts a typical phrase at
  velocity 0.7 at −20 LUFS, so its instruments start balanced against each
  other. std/basic and std/acoustic use other scales: in the trial, the nylon
  guitar needed `level = 0.7` to match a std/basic electric piano at `0.11`.
- Run `maac check` after each edit. Diagnostics name the object and field at
  fault; fix that field.

## 3. Listen through measurements

```sh
maac analyze main.maac --section regions --lead melody:out --json > analysis.json
maac analyze main.maac --source master:out --json   # faster, for loudness passes
maac analyze main.maac --images analysis/           # spectrogram and piano roll
maac analyze main.maac --window region:b --json     # one section, exact
maac analyze main.maac --window region:b --preroll 4 --json   # quick, approximate
```

- **Work section by section.** While you change one section, analyse it with
  `--window`; add `--preroll 4` for fast iterations. A preroll result is
  marked `approximate`, so finish with a whole-piece analysis.

- **Findings first.** Read `findings` before anything else. Fix every `error`
  and `warning`, or decide that one is intended. For example, a deliberately
  hard-panned mix triggers `stereo_imbalance`.
- **Balance.** Name the melody's node output with `--lead`. A `lead_under`
  warning means the median lead note sits 8.5 dB or more under the rest of
  its mix point, in the octave band where the lead is strongest. Raise the
  lead, or lower the parts that share its register. The lead source's `lead`
  object gives the margin per section: the trials' approved mixes sit at −2
  to +3 dB, and in a listening round the lead was buried at −10 dB. For the
  other parts, compare `max_short_term_lufs` of each `stem` per region.
- **Groove.** A `groove_mismatch` warning means parts disagree about swing.
  Usually a placement lacks the shared `groove` reference, or a part was
  written at hand-swung positions under a groove that swings them again.
- **Images.** Look at the PNGs. The piano roll shows form and register; the
  spectrogram shows density, low-end build-up and sections.
- **Duration.** Rendering runs at roughly real time, so batch several edits
  into one measurement pass.

## 4. Mix and master

- **Routing.** Use `send` buses into `fx.reverb/1` with `mix = 1`, plus a
  return gain.
- **Master.** End the chain with an optional glue `fx.compressor/1` (ratio
  2–3, attack about 20 ms), then `fx.limiter/1` as the project output. Its
  `gain` sets the loudness and its `ceiling` caps the true peak. Raising the
  gain by 1 dB raises the integrated loudness by just under 1 dB until the
  limiter works hard, so measure, adjust and measure again. `maac analyze`
  reports what each compressor and limiter takes per section in its
  `dynamics` object.
- **Ceiling margin.** Set the ceiling about 0.1 dB under the delivery's
  true-peak limit. Dither and integer encoding can lift peaks by a hair: on
  "Late Window" a −1 dB ceiling measured −0.99999 dBTP and failed a −1 dBTP
  limit.
- **Lookahead latency.** The limiter delays its output by its lookahead (1.5
  ms by default). On the master this is harmless if the `tail` covers it. A
  stem that must stay aligned with the limited master needs a matching
  `core.delay/1`.
- **Noise.** Steady broadband noise, such as hiss, is far more noticeable than
  its loudness suggests, especially after compression. In the trial, hiss 27
  dB under the music still annoyed the listener, so leave it out unless asked.
  `exposed_noise` reports a steady stem that no note plays when it fills a
  band in the quietest passages.
- **Target.** About -16 to -14 LUFS integrated at no more than -1 dBTP suits
  streaming; calm genres can sit lower.

## 5. Deliver

Add a `maac.production/1` extension with a delivery and its limits. Its schema
is built in, so no file is copied:

```maac
asset schema { kind = descriptor; builtin = "maac.production/1"; }
```

[`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac) shows the shape. Then run:

```sh
maac deliver main.maac --delivery release --output-dir out --profile song
```

The manifest's `check_status` must be `pass`. Give the listener the WAV and
ask for a verdict.

## 6. Pitfalls from the trial

| Symptom from the listener | Cause | Detection |
| --- | --- | --- |
| "The melody and the beat keep missing each other" | Some parts swung, others straight | `groove_mismatch`; count onset fractions from `query-events` |
| "A constant sssss in the background" | Low-level steady noise lifted by compression | `exposed_noise`; the source's `floor_lufs` and `block_range_lu` |
| A note never sounds, or a part is lost under another | A zero velocity or level; a part far under another in its register | `silent_note`, `buried_note`; `--notes` for each note |
| A part sounds out of tune | A sample's `root`, a detuned oscillator, a wrong transposition | `pitch_mismatch` gives the cents; ±1200 is an octave |
| "The instruments sound tacky" | std/basic synthesized timbres that ignore velocity | Not measurable; use std/studio and std/acoustic, and vary velocity |
| Lead melody gets lost | Melody 6 dB under the comping | `lead_under` with `--lead`; `buried_note` does not catch it |
| Lead drops in the quiet sections | Less amp `drive` also means less amp output | `lead_under` in those sections; give `synth.drive/1` a `reference`, or automate the level with the drive |
| The chorus does not lift | Glue and limiter flatten the louder section | Compare the master's `max_short_term_lufs` per region. Each processor's `dynamics.reduction_db` in the chorus minus the verse is what it takes from the lift. A flattened level alone is not the fault: in a listening round, a chorus that kept none of its lift still sounded fine because its arrangement built |
| "The acoustic guitar sounds cheap" | The std/acoustic/1.0.0 guitars start every string from the same full-band noise burst, at any velocity. Strummed in a full mix, they sound like a cheap synthesizer | Not measured. Define the guitar in the song, with velocity on the pluck's `hardness`, a pick `position` and body resonances, as [`examples/lantern/lantern.maac`](../examples/lantern/lantern.maac) does |
| "The guitar melody sounds like a synthesizer" | The electric guitar's `vibrato` control is one LFO on every note from note-on | Not measured. Set the control to 0; give held notes a `vibrato` with `delay` and `shape = up`, and bend some notes up from a whole step below (section 2). `pitch_mismatch` checks these notes against their expressed pitch at the pick |
| True peak over the limit after a sound change | Plucked attacks through compressors standing in for a limiter | `true_peak_over`; master through `fx.limiter/1` with its ceiling under the limit |
