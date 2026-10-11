# Electric guitar and piano: proposal

**Status:** accepted 2026-10-11; the owner accepted every
[decision](#decisions). Plan step 1, the string and drive additions, has
landed; see [results](#results-part-1).

## Why

The [T3 production trial](ai-production-trial-t3.md) ranked the instrument
palette first again (F17). On the folk-pop "Morning Ferry", the owner said
the instruments "still sound a bit like old game music". For the rock-ballad
version the AI had to design an amp chain inside the song, because no
library has an electric guitar. No library has an acoustic piano either, the
most common accompaniment instrument in pop. The inline amp also showed F22:
lowering its `drive` lowered its output, so every drive automation needed a
matching level automation.

This proposal adds an electric guitar and an acoustic piano to std/studio,
with the few building blocks they need. Like the
[palette proposal](instrument-palette-proposal.md), it adds general
processors and then builds instruments with them, rather than presets alone.

## What makes the plucked sounds dated

Every guitar in MaaC plays [`synth.pluck/1`](plucked-string.md), a
recirculating string. Its contract excludes "pluck position, excitation
hardness, a new body resonator, and velocity-dependent timbre". Each of these
exclusions removes a cue that a real string is being played:

1. **Every pluck starts from white noise.** At note-on the string's ring is
   filled with uniform random values, so a soft and a hard pluck have the same
   spectrum and differ only in level. A real pick or finger excites fewer
   high partials when played softly.
2. **There is no pluck position.** Plucking a string at 1/5 of its length
   removes every fifth harmonic and shapes the rest, and moving the pick
   changes the tone. White noise has none of this structure, which is part of
   what makes a plucked string sound like a chip-tune "pluck".
3. **There is no pickup position.** An electric guitar's pickup hears the
   string at one point, which notches the spectrum in the same way. The neck
   and bridge pickups sound different for this reason alone.
4. **The inline cabinet is one two-pole low-pass.** A guitar speaker falls
   off far more steeply above about 5 kHz, with a low resonance and a
   presence peak. Twelve decibels per octave leaves fizz, which T3 hid with an
   extra one-pole.

Items 1 to 3 are cheap to add to the string. Item 4 needs no new processor:
two filters in series and a band-pass in parallel make a cabinet.

These are hypotheses about the owner's verdict. The listening round in the
[plan](#plan) tests them.

## 1. String excitation and pickup: additions to `synth.pluck/1`

Three optional parameters, each with a default that leaves today's output
bit-identical. This follows the precedent of `curve` on `synth.adsr/1`:
previously invalid source becomes valid, no existing source changes meaning,
and std/acoustic/1.0.0 keeps its sound.

| Parameter | Meaning | Default; range | Rate |
| --- | --- | --- | --- |
| `hardness` | Brightness of the excitation: a one-pole low-pass on the initial fill, with 1 meaning no filter | 1; 1/100…1 | Captured at note-on |
| `position` | Pluck point as a fraction of the string. 0 means no position filter | 0; 0…1/2 | Captured at note-on |
| `pickup` | Pickup point as a fraction of the string. 0 means no pickup filter | 0; 0…1/2 | Sample |

**Excitation.** At note-on the fill is computed as today, then:

- Filtered by `y[i] = y[i-1] + hardness (x[i] - y[i-1])`, which is the
  identity at 1.
- Rescaled to the unfiltered fill's root mean square, so `hardness` changes
  tone and not level. Velocity still sets level.
- The string's first period, the `P = round(48000 / f)` cells the loop
  reads first, is combed circularly by `x[j] - x[(j - M) mod P]`, where `M`
  rounds `position × P` to whole samples. The harmonics at multiples of
  `P / M` then vanish exactly.

The rescale uses the first period's root mean square, since that is what the
string plays.

A modulation from `synth.velocity/1` to `hardness` gives a pluck that is
brighter when played harder.

**Pickup.** The output becomes `level × (v - v_K)`. Here `v_K` is the value
the loop will read `K = pickup × P` samples later, read from the existing
ring with the same fractional interpolation as the loop. The ring already
holds it, so no new memory is needed. Reading the past instead would need
more history than the ring keeps for low notes. `K` follows the pitch every
frame, so the notches stay on the same harmonics through a bend.

The implementation contract will state the exact order, rounding and
validation, as the plucked-string contract does. Work: the pickup read adds
one interpolated read per sample. A benchmark decides whether the 16-unit
charge must rise.

## 2. Level-compensated drive: `reference` on `synth.drive/1` (F22)

One optional parameter, default 0, which means off and keeps output
bit-identical. When `reference = r > 0`:

```text
y = level * (tanh(drive * x + bias) - tanh(bias)) / n
n = (tanh(drive * r + bias) - tanh(bias)) / r
```

A positive peak of amplitude `r` therefore leaves at `level × r` at every
drive setting. More drive adds harmonics and compression but keeps the peaks
of a typical signal where they were, so loudness stays roughly level. The
anti-aliased form divides the same way. When `reference > 0`, `drive` must be
positive, because `n` would otherwise be 0. Range: 0…1, sampled every frame.

This is a choice of normalization, not loudness matching: the loudness
still moves somewhat with drive. The [acceptance](#acceptance) bounds it.

## 3. Modal resonator bank: `synth.modal/1`

A piano string is stiff, so its partials are stretched:
`f_n = n f_1 √(1 + B n²)`. `B` rises from about 10⁻⁴ in the bass to 10⁻² in
the top octave. Each partial also decays at its own rate, and two or three
slightly detuned strings per note beat against each other. A recirculating
string cannot produce stretched partials. A sum of `synth.sine/1` nodes can,
but it needs an oscillator and an envelope per partial and per string: about
100 nodes per voice.

The palette proposal kept a modal bank in reserve as "the first thing to
revisit if the drums or the piano fall short". This proposal adds it as a
general processor, usable for bells, mallets and struck drums as well as the
piano.

`synth.modal/1` is voice-only, with one mono `in` for an excitation signal and
one mono `out`.

| Field | Meaning | Default; range |
| --- | --- | --- |
| `config.modes` | Number of modes, required | 1…64 |
| `config.strings` | Detuned copies of every mode | 1; 1…3 |
| `ratio` | Multiplier on the voice frequency | 1; 1/8…8 |
| `stretch` | `B` in `f_n = n f √(1 + B n²)`, with `f` the voice frequency × `ratio` | 0; 0…1/16 |
| `decay` | 60 dB decay time of the first mode | 2 s; 1/100…60 s |
| `damping` | Decay falls with mode number: `T_n = decay / (1 + damping (n - 1))` | 0; 0…4 |
| `position` | Strike point: mode `n` is weighted by `sin(π n position)`, and 0 means weight 1 | 0; 0…1/2 |
| `detune` | Spread between the copies, in cents: copy `k` is offset by `(k - (strings - 1)/2) × detune` | 0 ct; 0…50 ct |
| `level` | Output gain | 1; 0…16 |

Each mode and copy is a complex one-pole resonator:
`s ← R e^{iω} s + w x`, with `R = 10^(−3 / (T_n × 48000))` and output
`level × Σ Im(s) / strings`. An impulse of 1 starts each mode at amplitude
`w`, the position weight. Modes at or above 20 kHz contribute nothing, so
high notes use fewer modes. Coefficients are recomputed when a parameter
changes, as `synth.pluck/1` does, so pitch expression bends the whole bank.
State resets at note-on. The work charge is proportional to
`modes × strings` and is fixed by a benchmark, as the drive's was.

The excitation comes from existing nodes. A hammer is a short pulse from an
ADSR with zero attack and a decay of 1–4 ms, through a low-pass, plus a
little noise for the felt. Velocity shortens the pulse and opens the
low-pass, so a hard note is brighter. No new exciter node is needed.

## 4. The instruments: `std/studio/1.1.0`

A new library version adds two exports. `std/studio/1.0.0` stays byte for
byte as released, so existing songs keep their sound. `1.1.0` contains every
1.0.0 export unchanged, as std/basic/1.1.0 did when it added the kit.

| Export | Model | Velocity and key |
| --- | --- | --- |
| `electric_guitar` | Voice: a `synth.pluck/1` string with `position` and `pickup`, and an LFO for optional vibrato. Shared amp: a low cut, then a compensated `synth.drive/1` and a DC cut. The cabinet follows: a resonant high-pass near 90 Hz, two low-passes in series near 4.5 kHz (24 dB per octave), and a band-pass presence peak mixed in. Then the pan. | Harder picks are brighter (`hardness`) and drive the amp harder. Higher notes decay faster. |
| `piano` | Voice: a hammer pulse into a `synth.modal/1` bank of up to 48 modes and 2 strings, plus a low thump for the soundboard knock. Stereo by key, low notes left, as the player hears it. Shared: a body filter. | Harder notes are shorter, brighter pulses. Key sets `stretch`, `decay` and `damping`. The top octave and a half has no damper, so its release is long. |

The amp is in the shared graph, so a power chord's strings distort together
and intermodulate, as they do in a real amplifier. T3's inline instrument
worked this way.

**Controls.** These follow std/studio: `level`, `brightness` and `release`,
plus a few named character controls.

| Export | Controls |
| --- | --- |
| `electric_guitar` | `level`, `drive` (clean at 1, lead at 12), `tone` (cabinet cutoff), `pickup` (`bridge` 0.1 to `neck` 0.3, a number), `mute` (palm-mute damping), `pan`, `release`, `vibrato` |
| `piano` | `level`, `brightness` (hammer low-pass), `release`, `width` (stereo spread by key), `hammer` (pulse hardness), `tuning_spread` (`detune`) |

**Levels.** Each export is calibrated to −20 LUFS at velocity 0.7 on a
reference phrase, like the rest of std/studio. The guitar's phrase is the
"Morning Ferry" power-chord chorus at its default drive. The piano's is
four bars of ballad accompaniment across three octaves.

**Range.** Guitar E2–E6, the pluck's valid range. Piano A0–C8.

## Effect on "Morning Ferry"

The song's rhythm and lead guitars move to `studio.electric_guitar`. The
inline instrument, about 40 lines, goes away. With the compensated drive,
the `lead_gain` automation also goes away; the `lead_drive` automation
stays. The parts are re-levelled to the approved balance and delivered
again. The acoustic picking stays on std/acoustic.

There is no piano part in the song. The piano is auditioned on its own, and
the "Morning Ferry" verse is played on the piano as an A/B, not as a change
to the song.

## Plan

1. **String and drive.** The `synth.pluck/1` and `synth.drive/1` additions,
   with an implementation contract for each.
2. **Electric guitar.** An unreleased `electric_guitar` with auditions: T3's
   inline guitar against the new one at matched loudness, clean, crunch and
   lead, soft and hard. One listening round with the owner.
3. **"Morning Ferry".** Switch the song to the export, then an A/B against
   the approved master and the owner's verdict.
4. **Modal bank.** `synth.modal/1` with its contract.
5. **Piano.** An unreleased `piano` with auditions: low, middle and high
   registers; soft and hard; a ballad phrase. One listening round.
6. **Freeze.** `std/studio/1.1.0` with both exports, a delivery report, and
   updates to F17 and F22 in the T3 record.

The guitar comes first: it is smaller, it needs only additions to existing
processors, and it has a song to test in. The piano needs a new processor.

## Acceptance

- **Compatibility.**
  - Every existing test passes.
  - std/acoustic, std/studio/1.0.0, "Late Window" and the approved "Morning
    Ferry" master render bit-identically before the song switches.
- **String.**
  - `hardness < 1` lowers the spectral centroid of the first 50 ms
    monotonically and keeps the fill's root mean square.
  - `position = 1/5` puts the 5th, 10th and 15th harmonics at least 20 dB
    under their neighbours.
  - `pickup` notches are stable under a pitch bend.
  - Pitch accuracy keeps the plucked-string contract's 2-cent target.
- **Drive.**
  - With `reference`, a sine at amplitude `r` peaks at `level × r` within
    1e-9 at drive 1, 4, 16 and 64.
  - The guitar's reference phrase stays within ±2 LU across `drive` 1…16.
- **Modal bank.**
  - Mode frequencies match `n f √(1 + B n²)` within 0.1 cent.
  - Mode decay matches `T_n` within 1 %.
  - Modes at or above 20 kHz are absent.
  - A per-sample pitch sweep stays finite and bounded.
- **Instruments.**
  - Every export renders every semitone of its range at velocities 0.2, 0.5
    and 1.
  - The upper-band share rises with velocity.
  - Reference phrases land within ±1 LU of −20 LUFS.
  - `maac analyze` reads both as pitched: `pitch_mismatch` finds no note over
    50 cents. The piano's stretched partials are the check that matters.
- **Cost.**
  - A three-minute piano ballad with a typical part fits the song profile's
    10 billion work units with the rest of a band.
  - The switched "Morning Ferry" renders in no more than 1.5 times its current
    time.
- **Listening.** The owner's verdict on each instrument is recorded. The
  numbers show the processors behave as specified; only listening shows
  whether the "old game music" quality is gone.

## Results: part 1

Landed 2026-10-11. The exact rules are in the
[plucked-string contract](plucked-string.md#excitation-and-pickup-2026-10-11)
and the [saturation reference](instruments.md#saturation).

- **Nothing existing changed.** The approved "Morning Ferry" master, which
  uses `synth.pluck/1` and `synth.drive/1`, redelivers byte-identically, and
  every existing test passes.
- **String.**
  - `hardness` 0.5, 0.2 and 0.05 darken the fill step by step and keep the
    first period's root mean square within 1e-12.
  - At `position = 1/5`, the 5th, 10th and 15th harmonics sit 53 dB under
    their neighbours.
  - A `pickup` at 1/5 notches the same harmonics by 58 dB at 480 Hz, and by
    54 dB after a bend to 600 Hz.
  - Shaped strings stay within 2 cents at E2, E4 and E6.
  - Velocity modulates `hardness` through the note-on capture; the pluck
    joins the processors whose note-on parameters accept modulation.
- **Drive.** With `reference`, an input held at `reference` leaves at
  `level × reference` within 1e-12 at drive 1, 4, 16 and 64 and three biases.
  `reference = 0` matches the plain drive bit for bit. The loudness bound
  across `drive` is measured on the guitar in part 2.
- **Cost.** In a release benchmark, twelve ringing strings over 32 s took
  0.92 s of CPU time without a pickup and 0.95 s with one. That is under one
  unit of the 16-unit charge, so the charge is unchanged.
- **Changed from the proposal.** The position comb and the level rescale use
  the first period, `round(48000 / f)` cells, rather than the loop delay. The
  pickup reads the value the loop will read `K` samples later rather than an
  earlier one, so it needs no more memory than the ring.

## Cost and risk

- **The piano is the hardest acoustic instrument to synthesize.** A modal
  bank captures stretched partials, beating and per-partial decay. It misses
  sympathetic resonance, the sustain pedal and hammer–string contact detail.
  The likely result is "a convincing synthesized piano", not a sampled grand.
  If listening says it falls short, the next step is a soundboard model or
  sampled zones, which need recordings and a licence position.
- **Work budget.** "Morning Ferry" already uses about 5.7 of the song
  profile's 10 billion units. A 48-mode, 2-string piano costs far more per
  voice than any std/studio export. The acceptance's cost bound may force
  fewer modes in the treble, or a larger song profile.
- **Aliasing at high drive.** The palette proposal measured the drive's
  anti-aliasing as weaker near 24 kHz. A lead at drive 12 on high notes may
  sound harsh; the cabinet's steep low-pass hides most of it. Oversampling
  is the fallback.
- **The diagnosis may be wrong.** If the owner still hears game music after
  step 2, the cause lies elsewhere: the arrangement, the mix, or the
  std/studio drums and bass. The A/B is there to find that out early.

## Not proposed

- **Sustain pedal and sympathetic resonance.** Neither the language nor the
  instruments have a pedal. Authors lengthen `dur` instead. Revisit if the
  piano is approved and T4 needs pedalling.
- **Updating std/acoustic.** The steel and nylon guitars would gain from
  `hardness` and `position` too. That needs `std/acoustic/1.1.0`, a separate
  small follow-up once the guitar listening round shows the change helps.
- **Sampled instruments.** They need recordings and a licence position.
- **Amp effects beyond the cabinet**, such as a chorus, delay or tremolo
  pedal. The modulated delay remains a character-processor item.
- **Separate clean, crunch and lead exports.** One export covers them with
  `drive`; see decision 5.

## Decisions

The owner accepted every recommendation on 2026-10-11:

1. **Approach:** synthesized, with the additions above. Recommended over
   samples for the same reasons as the palette proposal: no recordings, no
   licence, deterministic output.
2. **String:** additive `hardness`, `position` and `pickup` parameters on
   `synth.pluck/1`, not a `synth.pluck/2`.
3. **Drive compensation (F22):** an additive `reference` parameter on
   `synth.drive/1`.
4. **Piano engine:** a general `synth.modal/1` bank. Recommended over a bank
   of oscillators and envelopes, which would cost about 100 nodes per voice,
   and over a piano-only processor, which would serve nothing else.
5. **Exports:** one `electric_guitar` whose `drive` spans clean to lead, and
   one `piano`, in `std/studio/1.1.0`, with `1.0.0` unchanged.
6. **Order:** guitar first, through "Morning Ferry", then the piano.
7. **Listening:** one A/B round per instrument, with the owner's verdict
   recorded, before the freeze. The owner declined to tune by ear in the
   palette work, but their T3 verdict on timbre was the trial's clearest
   signal. One comparative verdict per instrument is a small ask. If the
   owner prefers, the freeze can rest on the numbers alone, as for
   `1.0.0`.
