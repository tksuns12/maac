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
