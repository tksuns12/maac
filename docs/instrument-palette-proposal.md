# Instrument palette: synthesis primitives and a studio library

**Status:** accepted 2026-10-06; in progress. The owner chose the
synthesized direction over sampled instruments and character processors and
accepted every [decision](#decisions). Part 1, the note sources and the
envelope `curve`, has landed in the
[instrument contract](instruments.md#note-sources).

## Why

The [T2 production trial](ai-production-trial.md) ranked the instrument
palette first among the things to build (F14). The owner's verdicts on "Late
Window" ([`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac)) went from
"the instruments sound a bit tacky" to "the std/basic keys, bass, drums and
pad still sound dated". Mixing, mastering and timing problems were fixed
during the trial. The timbres were not, and they cap how good any track can
sound.

"Late Window" plays four std/basic instruments: `electric_piano`,
`finger_bass`, the `drums` kit with a separate `snare`, and `warm_pad`. The
fifth part, the std/acoustic nylon guitar, was the one change the owner heard
as an improvement.

## The existing position, and why presets cannot fix it

The [design principles](design-principles.md) put instruments in libraries
and new algorithms behind versioned engine boundaries. A better electric piano
would ordinarily be a new preset. The std/basic graphs show why a preset is
not enough: the dated sound comes from what the graph vocabulary cannot say.

1. **Velocity only changes loudness.** The designated envelope and velocity
   multiply the voice output once, and no graph node can read velocity. A
   hard note is a louder soft note. On an electric piano, a bass and drums,
   the change of timbre with playing strength is the main cue that a sound is
   played rather than generated. The basic electric piano's FM tine has a
   fixed 300 Hz depth at every velocity.
2. **No graph node can read the key either.** Higher piano and bass notes
   decay faster and sound darker relative to their pitch. Today a decay or a
   filter cutoff is the same for every note.
3. **Envelopes are linear.** Struck and plucked sounds decay exponentially. A
   straight-line decay over 1.8 s, as in `electric_piano`, holds too much
   level in the middle and then stops, which reads as synthetic.
4. **Filters are one-pole.** `synth.onepole/1` and `synth.highpass/1` slope
   at 6 dB per octave with no resonance. There is no filter character for a
   pad or a bass, and no band-pass to shape noise into a snare or a hat.
5. **Nothing is nonlinear.** No processor saturates, in instruments or in
   the production chain. Pickups, tape and analog circuits all add harmonics
   that grow with level, and part of the "played" quality comes from that.

`std/acoustic` improved the guitar for the same reason: it added an engine
algorithm, [`synth.pluck/1`](plucked-string.md), rather than a preset. This
proposal adds four general building blocks and then builds a library with
them. None of the four is specific to one instrument.

## 1. Note sources: `synth.velocity/1` and `synth.key/1`

Two voice-only source nodes, following `synth.timbre/1` and
`synth.pressure/1`: no inputs, parameters or configuration, and one mono
`out`.

| Source | Output | Changes |
| --- | --- | --- |
| `synth.velocity/1` | The note's velocity, 0…1 | Never; constant for the voice's life |
| `synth.key/1` | Octaves from C4: `log2(f / f_C4)`, with `f_C4 = 440 Hz * 2^(-9/12)` | Follows the voice's base frequency, including per-note pitch expression |

`f` is the base frequency the voice receives, before node ratios and
oscillator offsets, so `key` is −1 an octave below C4 under any tuning that
puts the note there. Both sources feed ordinary connections or signed-depth
modulation, with final bounds checks and no clipping:

```maac
node vel { type = "synth.velocity/1"; }
node tone { type = "synth.svf/1"; config = { channels = 1; mode = "lowpass"; }; params = { cutoff = 900Hz; }; }
modulate harder_brighter { from = &vel:out; to = &tone.params.cutoff; depth = 2400Hz; }
```

A modulation captured at note-on, such as an ADSR's `decay`, takes the value
at note-on. A kit passes each hit's velocity to its piece. The amplitude
still scales by velocity exactly once; the sources add timbre and leave that
rule alone.

## 2. Curved envelopes: `curve` on `synth.adsr/1`

`synth.adsr/1` gains an optional dimensionless parameter `curve`, default 0,
range 0…16, captured at note-on with `attack`, `decay` and `sustain`. For
`curve = c > 0`, the decay and release segments follow

```text
g(x) = (exp(-c x) - exp(-c)) / (1 - exp(-c)),   x = elapsed / duration in 0…1
decay:   S + (1 - S) g(x)
release: L g(x)        L = level at note-off
```

`g` runs from exactly 1 to exactly 0, so every segment still ends where it
ends today: the decay reaches the sustain level at `decay`, and a released
voice retires at `release`. Only the shape between changes. `curve = 0` is the
existing linear segment, bit for bit. With `curve = 7` a segment follows an
exponential fall for most of its length: it is 31 dB down at the halfway
point and 61 dB down at 90 %. Attack stays linear.

This joins `synth.adsr/1` rather than a new `synth.adsr/2`. The same
precedent added stereo and velocity layers to `synth.sample/1`: previously
invalid source becomes valid, no existing source changes meaning, and an
older host rejects the unknown parameter explicitly.

## 3. Resonant filter: `synth.svf/1`

A two-pole state-variable filter in the trapezoidal (topology-preserving)
form. It stays stable when the cutoff moves every sample, which modulation by
envelopes, LFOs, velocity and key requires.

| Field | Meaning | Default; range |
| --- | --- | --- |
| `config.channels` | 1 or 2, required | — |
| `config.mode` | `"lowpass"`, `"bandpass"` or `"highpass"`, required | — |
| `cutoff` | Hz, added to the key-tracked part | 1000 Hz; the sum strictly within 0…24000 Hz |
| `ratio` | Key tracking: cutoff is `note_hz * ratio + cutoff` | 0; 0…64, and 0 in a shared graph |
| `q` | Resonance; 0.707 is maximally flat | 707/1000; 1/10…40 |

`ratio` follows the oscillator convention, so a voice filter can keep the same
brightness relative to the pitch across the keyboard. Per sample and channel,
with `g = tan(pi * fc / 48000)` and `k = 1/q`:

```text
a1 = 1 / (1 + g (g + k));  a2 = g a1;  a3 = g a2
v3 = x - s2;  v1 = a1 s1 + a2 v3;  v2 = s2 + a2 s1 + a3 v3
s1 = 2 v1 - s1;  s2 = 2 v2 - s2
lowpass = v2;  bandpass = k v1;  highpass = x - k v1 - v2
```

The band-pass output has unity gain at the cutoff, so raising `q` narrows a
band of noise without making it louder. State resets at note-on for a voice
filter and at render reset for a shared one, as for the one-pole filters. The
filter is linear and cannot self-oscillate; `q` is bounded at 40.

## 4. Saturation: `synth.drive/1`

A memoryless `tanh` curve with an offset for asymmetry:

```text
u = drive * x + bias
y = level * (tanh(u) - tanh(bias))
```

| Parameter | Meaning | Default; range |
| --- | --- | --- |
| `drive` | Input gain into the curve | 1; 0…64 |
| `bias` | Offset; nonzero adds even harmonics | 0; −4…4 |
| `level` | Output gain | 1; 0…16 |

`config.channels` is 1 or 2. Subtracting `tanh(bias)` keeps silence silent.
An asymmetric curve also shifts the average level of a loud signal, and a
`synth.highpass/1` after the drive removes that.

A naive curve aliases: the harmonics it adds above 24 kHz fold back into the
audible band. The processor therefore uses first-order antiderivative
anti-aliasing, as in Parker, Zavalishin and Le Bivic, "Reducing the
aliasing of nonlinear waveshaping using continuous-time convolution"
(DAFx-16). With `F(u) = ln cosh(u)` evaluated in an overflow-safe form,

```text
y[n] = level * ((F(u[n]) - F(u[n-1])) / (u[n] - u[n-1]) - tanh(bias))
```

with `tanh((u[n] + u[n-1]) / 2)` in place of the quotient when
`|u[n] - u[n-1]| < 1e-6`. This adds half a sample of delay and needs no
oversampling filter whose design would also have to be fixed. The previous
`u` is the only state, reset at note-on or render reset like the filters.

## Shared rules for the new processors

- Sources are voice-only; the filter and drive are legal in voice and shared
  graphs. The processing graph stays a DAG with no feedback.
- Parameters are exact rationals until the DSP boundary and are sampled each
  frame, except `curve`. Invalid evaluated values fail; nothing clamps.
- Each processor's output and state preview without committing, as existing
  processors do for note-on and release captures.
- Each gets a fixed per-sample work weight, measured against the existing
  processors and recorded in the [capability matrix](capabilities.md). The
  new kinds join the instrument resources that plan versions 2–7 share, with
  no new plan version, as `synth.pluck/1` did.
- Existing plans, `std/basic` and `std/acoustic` render bit-identically.

## 5. The library: `std/studio/1.0.0`

A third built-in collection, beside `std/basic` and `std/acoustic`, made
only from the processors above and the existing ones. It needs no recordings,
downloads or library directory.

| Export | Model | Velocity and key |
| --- | --- | --- |
| `electric_piano` | A tine piano: a strong fundamental with a long curved decay, plus short inharmonic partials at the attack. An asymmetric drive stands in for the pickup, and a stereo tremolo runs in the shared graph. | Harder notes bark: more partials and more drive. Higher notes decay faster. |
| `bass` | A finger bass on the existing `synth.pluck/1` string, with a key-tracked low-pass and a little drive. | Harder notes are brighter and less damped. The release mutes the string. |
| `pad` | Four detuned saws in a stereo pair, through a key-tracked resonant low-pass that drifts slowly. Each key starts its LFO at a different phase. | Velocity opens the filter. |
| `drums` | A kit with the same eight keys as the `std/basic/1.1.0` kit: `kick`, `snare`, `clap`, `hat`, `open_hat`, `low_tom`, `high_tom`, `crash`. The hats choke each other. Pieces are exported too. | Harder hits are brighter, and the kick's pitch drop and click grow. |

The kick uses a curved pitch drop and a drive for weight. The snare uses two
body modes plus noise through a band-pass for the wires. The hats and crash
use six inharmonic square waves through band-pass and high-pass filters, as
analog drum machines did. The table describes the starting design; listening
rounds decide the details.

The pitched exports also map per-note [timbre expression](timbre-expression.md)
to brightness, so authored timbre curves work on them. Their controls follow
std/basic: `level`, `brightness`, `release` and `pan`, plus a few named
character controls such as `bark`, `tremolo_depth` and `motion`. The kit
keeps the `<piece>_level`, `_pan` and `_brightness` pattern.

Keeping the std/basic kit keys means switching "Late Window" to the new drums
is a change of import, not of patterns.

### Calibrated levels (F15)

In "Late Window", `level` runs from 0.05 on the pad to 0.7 on the guitar.
That spread is not a mix decision: it is the libraries' uncalibrated scales
(F15). Every `std/studio` default is set so that a reference phrase for the
instrument, played at velocity 0.7, measures −20 LUFS integrated within
±1 LU. The reference phrases and their measurements go in the library's
delivery report. This fixes F15 inside `std/studio` only; it does not
re-scale existing libraries.

### Freezing

A released library version keeps its sound. `std/studio/1.0.0` therefore
stays unreleased while the listening rounds change it. It is frozen, with
its hash, only once the owner approves it.

## Effect on "Late Window"

The track moves its electric piano, bass, drums, snare and pad to
`std/studio`. The notes, groove and arrangement do not change. The nylon
guitar stays. The mix is re-levelled and the master re-checked with
`maac analyze` and `maac deliver`.

Cost matters as well as sound. The richer graphs cost more per voice. Today
the track renders in 26.7 s on an Apple M1 (0.34× real time), and its
instruments use 2.6 billion of the song profile's 10 billion work units.

## Plan

1. **Primitives, part 1.** The note sources and the envelope `curve`.
2. **Primitives, part 2.** `synth.svf/1` and `synth.drive/1`.
3. **Library.** Unreleased `std/studio` with auditions that put std/basic and
   std/studio side by side on the same notes at matched loudness: low, middle
   and high notes; soft and hard. One or more listening rounds with the
   owner.
4. **"Late Window" on std/studio.** An A/B against the current master at
   matched loudness and the owner's verdict. Then the freeze, a delivery
   report, and an update to F14 and F15 in the trial report.

## Acceptance

- **Note sources.** Velocity and key read back exactly in a graph that routes
  them to the output. They follow per-note pitch, and a kit passes each hit's
  velocity to its piece.
- **Curve.**
  - `curve = 0` renders bit-identically to today.
  - Curved segments reach the sustain level and zero at exactly the existing
    frames, and voices retire on the same frames.
- **Filter.**
  - Measured magnitude responses match the bilinear-transform transfer
    function within a stated tolerance at fixed frequencies, for each mode
    and several values of `q`.
  - A per-sample cutoff sweep across the whole range stays finite and
    bounded.
- **Drive.**
  - Slow signals match `tanh` within a stated tolerance.
  - On a fixed 5 kHz fixture with high drive, the anti-aliased output's
    folded components are measurably lower than a naive curve's. The fixture
    and the measured reduction are recorded.
- **Compatibility.** Existing tests pass. `std/basic` and `std/acoustic`
  auditions and the current "Late Window" master render bit-identically.
- **Library.**
  - Every export renders across its suggested range at velocities 0.2, 0.5
    and 1 without errors.
  - The octave-band measurements in `maac analyze` show the upper-band share
    rising with velocity for every pitched export and every drum.
  - Reference levels land within ±1 LU of −20 LUFS.
- **"Late Window".**
  - It renders under the song profile at no more than twice today's render
    time.
  - `maac deliver` passes.
  - The owner's listening verdict is recorded. The owner's ears decide
    whether F14 is fixed; the numbers above only show that the primitives
    behave as specified.

## Not proposed

- **A modal resonator bank.** Sine oscillators with curved envelopes already
  give the sound of struck modes. A bank would add a noise-excited response
  and re-strike behaviour. It is the first thing to revisit if the drums or
  the piano fall short in listening.
- **A modulated delay** for chorus, ensemble and tape wow. This belongs to
  the character processors, which the owner put after this proposal. Tremolo
  and auto-pan already work with an LFO in a shared graph.
- **Sampled instruments.** They need recordings and a licence position.
- **A velocity-to-loudness curve.** Amplitude stays proportional to velocity.
- **Per-voice randomness** such as analog drift or round-robin variation. It
  would need seeded per-voice variation with its own determinism rules.
- **Changes to `std/basic` or `std/acoustic`.** Their released versions keep
  their sound.

## Decisions

The owner accepted every recommendation on 2026-10-06:

1. **Primitives:** the four above. A modal bank and a modulated delay wait
   for listening evidence.
2. **Envelope shape:** an additive `curve` parameter on `synth.adsr/1`, not a
   new `synth.adsr/2`.
3. **Library name:** `std/studio/1.0.0`.
4. **Exports:** `electric_piano`, `bass`, `pad`, and a `drums` kit with the
   std/basic kit's keys.
5. **Levels:** −20 LUFS at velocity 0.7 on each reference phrase.
6. **Order:** primitives, then the library through listening rounds, then
   "Late Window". The library is frozen only after approval.
