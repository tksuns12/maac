# AI producer guide

How an AI can produce a finished track with MaaC: write source, render it,
perceive it through measurements and images, revise, master and deliver.
Every step below was used in the [T2 production trial](ai-production-trial.md),
and the pitfalls are ones that trial hit.

You cannot hear your own render. `maac analyze` is your measurement of what
you would otherwise hear, and the person you work for is the final judge of
anything a number cannot settle: timbre, annoyance and feel. Ask them to
listen before you call a track finished.

## 1. Plan before writing

- **Instruments.** Start from `std/studio/1.0.0`
  (`maac instruments --library std/studio/1.0.0`): its electric piano, bass,
  pad and `drums` kit change timbre with velocity, and their levels share one
  calibrated scale. Use the `std/acoustic/1.0.0` guitars for guitar parts.
  std/basic covers other roles, such as organ, strings, flute and leads, with
  older synthesized timbres that ignore velocity except for loudness.
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
  Give `velocity` or `onset_offset` a list to voice or strum it.
- Write one-bar chord patterns and assemble progressions with `use`. Place
  progressions with `count` for repeats.
- Play drums from one kit node: import `std/studio/1.0.0`, add
  `node drums { instrument = &studio.drums; }`, and write `hit` leaves such as
  `hit k1 { at = 0q; key = "kick"; velocity = 0.85; }`. Keys are `kick`,
  `snare`, `clap`, `hat`, `open_hat`, `low_tom`, `high_tom` and `crash`; set
  each drum with `kick_level`, `hat_pan` and so on. A kit has one output, so
  a drum that needs its own send or effect stays a separate instrument node.
- Connect everything to an explicit stereo `core.sum/1` bus.
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
maac analyze main.maac --section regions --json > analysis.json
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
- **Balance.** Compare `max_short_term_lufs` of each `stem` per region. In the
  trial, a lead melody 6 dB under the comping did not read as a lead, so aim
  for the lead within about 2 dB of the main accompaniment.
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
  limiter works hard, so measure, adjust and measure again.
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
- **Target.** About -16 to -14 LUFS integrated at no more than -1 dBTP suits
  streaming; calm genres can sit lower.

## 5. Deliver

Add a `maac.production/1` extension with a delivery, its limits and a pinned
copy of `production.schema.json`. [`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac)
shows the shape. Then run:

```sh
maac deliver main.maac --delivery release --output-dir out --profile song
```

The manifest's `check_status` must be `pass`. Give the listener the WAV and
ask for a verdict.

## 6. Pitfalls from the trial

| Symptom from the listener | Cause | Detection |
| --- | --- | --- |
| "The melody and the beat keep missing each other" | Some parts swung, others straight | `groove_mismatch`; count onset fractions from `query-events` |
| "A constant sssss in the background" | Low-level steady noise lifted by compression | Not measured yet; avoid steady noise |
| "The instruments sound tacky" | std/basic synthesized timbres that ignore velocity | Not measurable; use std/studio and std/acoustic, and vary velocity |
| Lead melody gets lost | Melody 6 dB under the comping | Per-stem `max_short_term_lufs` by region |
| True peak over the limit after a sound change | Plucked attacks through compressors standing in for a limiter | `true_peak_over`; master through `fx.limiter/1` with its ceiling under the limit |
