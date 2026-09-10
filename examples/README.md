# Native sample kit

[`core-kit.maac`](core-kit.maac) arranges 24 native hits using three small,
original drum samples. The groove accelerates and slows down with a shared
tempo map; kit level automation controls its dynamics. Kick and snare assets
run at 24 kHz, and the hi-hat runs at 48 kHz. The mono output is 208,158 frames
at 48 kHz, including a quarter-second tail.

Run from the repository root:

```sh
maac check examples/core-kit.maac --project-root .
maac build examples/core-kit.maac --project-root . -o core-kit.wav --format pcm16
maac compile examples/core-kit.maac --project-root . -o core-kit.plan.json
maac render core-kit.plan.json -o core-kit-from-plan.wav --format pcm16
```

The plan embeds its samples for source-free replay. The external
[`generate_samples.py`](sounds/core-kit/generate_samples.py) documents how the
assets were made; rendering needs only the committed PCM files or retained plan.
See the [kit contract](../docs/core-kit.md) for raw format, playback and limits.

# Musical audio warping

[`warp-rate.maac`](warp-rate.maac) combines two native warp-rate clips, a rate
clip, two kit hits, and a pitched note through an explicit mixer, reverb, and
master gain. It reuses the original core-kit kick PCM. Piecewise musical anchors
and a tempo ramp change playback rate and pitch; the final clip continues into
the declared tail and crosses a tempo change after score end. The V6 plan
embeds the PCM for source-free replay.

```sh
maac build examples/warp-rate.maac --project-root . -o warp-rate.wav --format pcm16
maac compile examples/warp-rate.maac --project-root . -o warp-rate.plan.json
maac render warp-rate.plan.json -o warp-rate-from-plan.wav --format pcm16
```

# Core modulation

[`core-modulation.maac`](core-modulation.maac) uses score-clock and seconds-clock
LFOs to vary gain, filter cutoff, and pan. An automated constant combines control
signals alongside a pitched note, a kit hit, and three audio clips. The stereo
output is 109,688 frames at 48 kHz, including its tail. Its V7 plan embeds the
assets for source-free replay.

```sh
maac check examples/core-modulation.maac --project-root .
maac build examples/core-modulation.maac --project-root . -o core-modulation.wav --format pcm16
maac compile examples/core-modulation.maac --project-root . -o core-modulation.plan.json
maac render core-modulation.plan.json -o core-modulation-from-plan.wav --format pcm16
```

See the [modulation contract](../docs/core-modulation.md) for waves, clocks,
additive amounts, and supported continuous parameters.

# Event-rate modulation

[`event-rate-modulation.maac`](event-rate-modulation.maac) modulates attack at
note-on and release at note-off for a core sine and a reusable instrument.
The self-contained three-note phrase renders 50,400 mono frames at 48 kHz,
including its 50 ms tail. Its V7 plan replays without the source file.

```sh
maac build examples/event-rate-modulation.maac -o event-rate.wav
maac compile examples/event-rate-modulation.maac -o event-rate.plan.json
maac render event-rate.plan.json -o event-rate-from-plan.wav
```

# Per-note pressure mappings

[`internal-event-modulation.maac`](internal-event-modulation.maac) uses each
note's initial pressure to set ADSR attack and a per-voice LFO to set release
at note-off. Three overlapping notes demonstrate independent captures without
imports or assets.

```sh
maac build examples/internal-event-modulation.maac -o internal-event.wav
maac compile examples/internal-event-modulation.maac -o internal-event.plan.json
maac render internal-event.plan.json -o internal-event-from-plan.wav
```

[`internal-phase-modulation.maac`](internal-phase-modulation.maac) captures per-note
expression into voice oscillator and LFO phase at note-on.

```sh
maac build examples/internal-phase-modulation.maac -o internal-phase.wav
maac compile examples/internal-phase-modulation.maac -o internal-phase.plan.json
maac render internal-phase.plan.json -o internal-phase-from-plan.wav
```

[`internal-reset-modulation.maac`](internal-reset-modulation.maac) captures shared
LFO phase before notes begin, using authored controls and silent shared input.

```sh
maac build examples/internal-reset-modulation.maac -o internal-reset.wav
maac compile examples/internal-reset-modulation.maac -o internal-reset.plan.json
maac render internal-reset.plan.json -o internal-reset-from-plan.wav
```

[`reset-rate-modulation.maac`](reset-rate-modulation.maac) captures a top-level
control graph into instrument reset controls before shared-graph initialization.

```sh
maac build examples/reset-rate-modulation.maac -o reset-rate.wav
maac compile examples/reset-rate-modulation.maac -o reset-rate.plan.json
maac render reset-rate.plan.json -o reset-rate-from-plan.wav
```

[`core-fader.maac`](core-fader.maac) applies dB gain with automation and additive
control modulation.

```sh
maac build examples/core-fader.maac -o core-fader.wav
maac compile examples/core-fader.maac -o core-fader.plan.json
maac render core-fader.plan.json -o core-fader-from-plan.wav
```

[`core-matrix.maac`](core-matrix.maac) maps channels with explicit coefficient rows.

```sh
maac build examples/core-matrix.maac -o core-matrix.wav
maac compile examples/core-matrix.maac -o core-matrix.plan.json
maac render core-matrix.plan.json -o core-matrix-from-plan.wav
```

[`core-delay.maac`](core-delay.maac) creates a feedback echo with an explicit frame delay.

```sh
maac build examples/core-delay.maac -o core-delay.wav
maac compile examples/core-delay.maac -o core-delay.plan.json
maac render core-delay.plan.json -o core-delay-from-plan.wav
```

[`pressure-expression.maac`](pressure-expression.maac) defines an inline stereo
instrument whose pressure source raises oscillator level and filter cutoff.
Independent timbre also controls cutoff; one note combines all four expression
kinds: pitch, gain, timbre, and pressure. Three overlapping notes demonstrate
normalized, seconds, and score pressure clocks, including positive exponential
interpolation. No imports or assets are required.

```sh
maac check examples/pressure-expression.maac
maac build examples/pressure-expression.maac -o pressure-expression.wav
maac compile examples/pressure-expression.maac -o pressure-expression.plan.json
maac render pressure-expression.plan.json -o pressure-expression-from-plan.wav
```

Expected output is 91,200 stereo frames at 48 kHz (1.9 seconds including the
400 ms tail, which covers the 300 ms release). The [pressure guide](../docs/pressure-expression.md)
explains explicit opt-in, mappings, release holding, and shared limits.
Frozen basic/acoustic instruments do not opt in. No human listening approval
is claimed.

# Per-note timbre mappings

[`timbre-expression.maac`](timbre-expression.maac) defines two inline custom
instruments with explicit timbre sources: a filtered oscillator and a plucked
string. Two notes overlap on each instrument and vary independently; timbre
curves cover all three clocks, positive exponential interpolation, and simultaneous pitch,
gain, and timbre. It needs no external library or asset file.

```sh
maac check examples/timbre-expression.maac
maac build examples/timbre-expression.maac -o timbre-expression.wav
maac compile examples/timbre-expression.maac -o timbre-expression.plan.json
maac render timbre-expression.plan.json -o timbre-expression-from-plan.wav
```

Expected output is 91,200 stereo frames at 48 kHz (1.9 seconds including tail).
The [timbre guide](../docs/timbre-expression.md) explains authored mappings,
receiver opt-in, curve limits, and state continuity. Frozen basic/acoustic
instruments do not accept timbre expression.

# Per-note instrument pitch

[`instrument-pitch.maac`](instrument-pitch.maac) overlaps independently bending
piano and acoustic guitar notes, including piecewise-linear vibrato, all three
curve clocks, and simultaneous pitch and gain. Its embedded imports are standalone.
Build it with `maac build examples/instrument-pitch.maac -o instrument-pitch.wav`.
Expected output is 91,200 stereo frames at 48 kHz (1.9 seconds including tail).
The [instrument pitch guide](../docs/instrument-pitch.md) explains frequency
limits, state continuity, and retained-plan rendering.

# Per-note gain swells and fades

[`gain-expression.maac`](gain-expression.maac) combines pitch and gain on one
note while a second voice rises exponentially. Build it with
`maac build examples/gain-expression.maac -o gain-expression.wav`.
The [gain guide](../docs/gain-expression.md) explains curves and voice behavior.

# Per-note instrument gain

[`instrument-gain.maac`](instrument-gain.maac) overlaps independently shaped
piano and acoustic guitar notes using embedded imports. Build it with
`maac build examples/instrument-gain.maac -o instrument-gain.wav`.
The [instrument gain guide](../docs/instrument-gain.md) explains zero-gain
state, release holding, and standalone retained-plan rendering.

# Per-note pitch bends

[`pitch-expression.maac`](pitch-expression.maac) demonstrates two overlapping
notes bending independently on `core.sine/1`. Build it with
`maac build examples/pitch-expression.maac -o pitch-expression.wav`.
The [pitch guide](../docs/pitch-expression.md) explains curve clocks and limits.

# Built-in instrument examples

[`basic/`](basic/) contains one standalone audition for each of the 24 exports
in `std/basic/1.0.0`, plus [`full_band.maac`](basic/full_band.maac). Individual
auditions last 3.8 seconds; the full band lasts 9.8 seconds. Every file needs
only the installed executable, including when copied outside this checkout.

```sh
maac instruments
maac instruments kick --json
maac build examples/basic/nylon_guitar.maac -o guitar.wav --format pcm16
maac build examples/basic/full_band.maac -o full-band.wav --format pcm16
```

All sounds are synthesized interpretations. Pitched auditions demonstrate
overlap and dynamics. Drums use separate instrument exports with C2 triggers;
their note gates and release control damping. The full band explicitly routes
keys, guitar, bass, flute, pad, kick, snare and hi-hat into a stereo sum.

See the [basic instrument guide](../docs/basic-instruments.md) for names, pitch
ranges, gate recommendations, controls and the frozen-version contract.
Numerical rendering checks do not constitute human listening approval.

# Acoustic guitar authoring

The separate `std/acoustic/1.0.0` collection uses MaaC's recirculating string
processor and existing body EQ. Its examples passed fresh installed-executable
checks outside the checkout. [`acoustic/`](acoustic/) contains
7.5-second auditions for nylon, steel and muted guitar, plus the 5.7-second
[`fingerpicked_phrase.maac`](acoustic/fingerpicked_phrase.maac). The
[acoustic guitar guide](../docs/acoustic-guitars.md)
contains a complete composition and six-control explanation; the existing
basic auditions above retain their frozen sounds.

```sh
maac instruments --libraries
maac instruments nylon_guitar --library std/acoustic/1.0.0 --json
maac build examples/acoustic/nylon_guitar.maac -o nylon.wav --format pcm16
maac build examples/acoustic/fingerpicked_phrase.maac -o fingerpicked.wav --format pcm16
```

# Reusable starter sounds

`sounds/studio.maac` is a small local MaaC library used by
`reusable.maac`. It contains three instruments:

- `fm_bell` uses a per-voice sine modulator, a carrier, and an amplitude ADSR.
  Its shared graph filters and pans the summed voices. `soft_bell` and
  `glass_bell` vary release, modulation amount, brightness, and pan.
- `pad` scans three adjacent frames in `sounds/colors.wav`. Its public
  `position` control is sample-rate automatable; the example moves it across
  the table with a score-clock curve. `warm_pad` and `bright_pad` select
  useful starting colors.
- `bass` runs a band-limited saw through a per-voice one-pole filter.
  `deep_bass` and `pluck_bass` vary cutoff, level, and release.

The composition creates two independent `fm_bell` instances with different
presets and overrides, then combines them with the pad and bass through
explicit tracks, placements, and stereo routing.

Check the library and build the composition from the repository root:

```sh
maac check examples/sounds/studio.maac
maac build examples/reusable.maac -o reusable-output.wav
```

The checked-in WAV is authored source data, not rendered output. Recreate it
with Python's standard library:

```sh
python3 examples/sounds/generate_table.py
shasum -a 256 examples/sounds/colors.wav
shasum -a 256 examples/sounds/studio.maac
```

The generator writes three 512-sample mono PCM16 cycles from declared Fourier
amplitudes. It does not normalize their peaks. Its byte identity depends on the
explicit little-endian signed 16-bit packing and the deterministic WAV header.
After changing the generator or table, update the WAV pin in `studio.maac`,
calculate the library hash again, and update the import pin in
`reusable.maac`, in that order.

Automated checks establish deterministic, finite, nonsilent output and a
PCM-safe peak for this fixture. No human listening approval is claimed.
