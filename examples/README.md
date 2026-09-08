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
