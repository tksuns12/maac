# Built-in basic instruments

`std/basic/1.0.0` supplies 24 stereo instruments from the installed MaaC executable.
A composition needs only its own `.maac` text: no sound-library directory,
recordings, downloaded samples, or network resolution. These are synthesized
interpretations made from oscillator, envelope, noise, filter and mixing graphs;
acoustic names describe a musical role, not a recording or realism guarantee.

Discover the collection and inspect one instrument:

```sh
maac instruments
maac instruments mellow_piano
maac instruments mellow_piano --json
```

The detailed result includes its description, pitch and note-duration guidance,
exact control defaults and ranges, and a complete runnable source example.
JSON list results use `catalog`; detailed results use `instrument`. Exact
rational control values are serialized as strings. Discovery reports the
embedded source identity and SHA-256; use that record instead of copying a
potentially stale hash from a guide.

## First sound

Save this complete composition as `piano.maac`:

```maac
maac 1;
project song {
  score = [0q, 4q]; rate = 48000Hz;
  tempo = &clock; meter = &metre; output = &piano:out; tail = 1s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
import basic { builtin = "std/basic/1.0.0"; }
node piano { instrument = &basic.mellow_piano; }
pattern phrase {
  length = 4q;
  note c { at = 0q; dur = 1q; pitch = C4; velocity = 0.6; }
  note e { at = 1q; dur = 1q; pitch = E4; velocity = 0.5; }
  note g { at = 2q; dur = 3/2q; pitch = G4; velocity = 0.6; }
}
track notes { target = &piano:events; }
place once { pattern = &phrase; track = &notes; at = 0q; }
```

```sh
maac check piano.maac
maac compile piano.maac -o piano.performance.json
maac render piano.performance.json -o piano.wav --format pcm16
```

Copy any [individual audition](../examples/basic/) to an otherwise empty
directory and build it with the installed executable. Each audition lasts
3.8 seconds and demonstrates dynamics; pitched examples include overlap.
The [full band](../examples/basic/full_band.maac) lasts 9.8 seconds and combines
keys, guitar, bass, flute, pad, kick, snare and hi-hat through explicit stereo
routing. Its drum parts have separate instrument instances and event tracks.

```sh
maac build examples/basic/steel_guitar.maac -o steel-guitar.wav --format pcm16
maac build examples/basic/full_band.maac -o full-band.wav --format pcm16
```

## Musical starting points

Pitch ranges are conservative authoring guidance, not MIDI key switches or
compiler-enforced instrument ranges. MIDI labels use C4 = 60. Note durations
are gate lengths in seconds; tempo determines their equivalent `q` duration.
The catalog gives each instrument's exact current defaults.

| Instrument | Suggested pitches (MIDI) | Note duration | Character |
| --- | --- | --- | --- |
| `mellow_piano` | C2–C7 (36–96) | 0.15–2.5 s | Soft body and gentle FM attack |
| `bright_piano` | C2–C7 (36–96) | 0.1–2.5 s | Brighter harmonic attack |
| `electric_piano` | C2–C7 (36–96) | 0.15–3 s | Rounded body with an FM tine |
| `organ` | C2–C6 (36–84) | 0.15–6 s | Sustained additive harmonics |
| `nylon_guitar` | E2–E6 (40–88) | 0.08–1.5 s | Rounded, decaying pluck |
| `steel_guitar` | E2–E6 (40–88) | 0.08–1.5 s | Brighter pluck with a longer decay |
| `muted_guitar` | E2–E6 (40–88) | 0.08–1.5 s | Quickly damped pluck; most energy in 0.16 s |
| `finger_bass` | E1–G4 (28–67) | 0.15–1.5 s | Rounded plucked low end |
| `pick_bass` | E1–G4 (28–67) | 0.1–1.2 s | More pronounced pluck attack |
| `sub_bass` | C1–C4 (24–60) | 0.25–3 s | Smooth sustained low end |
| `synth_bass` | E1–G4 (28–67) | 0.15–2 s | Harmonic synthesized bass |
| `strings` | C2–C6 (36–84) | 0.3–6 s | Sustained synthesized string blend |
| `flute` | C4–C7 (60–96) | 0.15–4 s | Soft synthesized wind lead |
| `bell` | C3–C7 (48–96) | 0.2–3 s | Decaying FM bell |
| `warm_pad` | C2–C6 (36–84) | 1.5–8 s | Slowly developing sustained bed |
| `synth_lead` | C2–C6 (36–84) | 0.08–3 s | Sustained harmonic melody |

Drums use fixed oscillator tuning and unpitched noise. Use C2 (MIDI 36) as a
consistent trigger for every drum; changing the note pitch does not select a
kit piece or retune its body. Select the instrument export instead. Ordinary
note-off damps the sound; note duration and `release` both matter. There is no
implicit one-shot lifetime, hi-hat choke group, or drum-map routing.

| Drum | Starting gate | Useful gate range |
| --- | --- | --- |
| `kick` | 100 ms | 20–250 ms |
| `snare` | 100 ms | 25–240 ms |
| `clap` | 120 ms | 40–280 ms |
| `closed_hat` | 35 ms | 10–100 ms |
| `open_hat` | 220 ms | 80–900 ms |
| `low_tom` | 160 ms | 35–450 ms |
| `high_tom` | 120 ms | 25–340 ms |
| `crash` | 350 ms | 80–1800 ms |

## Common controls and voices

Every export has exactly these public controls:

| Control | Meaning | Allowed values | Rate |
| --- | --- | --- | --- |
| `level` | Overall instrument output gain, including all layers | 0–16 | Sample |
| `brightness` | Output one-pole low-pass cutoff | Strictly above 0 Hz and below 24000 Hz | Sample |
| `release` | Amplitude ADSR release after note-off | 0–1800 s | Captured at note-off |
| `pan` | Equal-power stereo position | −1 left, 0 center, +1 right | Sample |

For example, replace the `piano` node above with:

```maac
node piano {
  instrument = &basic.mellow_piano;
  config = { voices = 8; };
  params = { level = 0.12; brightness = 3500Hz; release = 300ms; pan = -0.2; };
}
```

Defaults, presets, then instance parameters apply in that order. Public controls
can use ordinary automation at their declared rates. `brightness` is a cutoff
in hertz, not a normalized 0–1 macro. Setting `level = 0` silences every layer.
`pan = 0` produces two channels; it does not add a room or stereo ensemble.

An instance defaults to 64 voices. Small examples explicitly request four or eight;
choose sufficient capacity for overlapping gates and release tails. The engine
reports overflow explicitly instead of stealing voices. A zero-sustain pluck
still occupies its voice until its note-off release retires it. The score end
releases held notes, and the project `tail` sets the final render boundary.

Start with the supplied conservative levels and moderate note velocities.
Polyphony, panning, brighter cutoffs and increased gain can change peaks;
PCM16 export rejects overload rather than limiting or normalizing the mix.
Generated auditions and numerical checks are separate from human listening;
no listening approval or cross-platform bitwise audio identity is claimed.

## Version and embedding contract

The import selects the exact identity `std/basic/1.0.0`; version aliases, version
fallback, registry lookup and filesystem shadowing are unsupported. This
version's source bytes are frozen when released. Changes to timbre defaults,
noise seeds or graphs require a new library version. The authored source is
[stdlib/basic/1.0.0.maac](../stdlib/basic/1.0.0.maac), embedded into the executable.

Resolution records it as `@builtin/std/basic/1.0.0.maac` with its exact SHA-256.
That namespace is reserved. Built-in source bytes, source objects, files and
import depth count toward the existing bundle limits; embedding is not a
resource-limit exemption. Multiple aliases reuse the same source identity.

Compiled version 2 plans embed the library instrument graphs, resolved noise
seeds and source/dependency provenance. Rendering a retained plan needs neither
the composition nor a sound-library directory and does not look up a newer
catalog. The [instrument specification](instruments.md) describes processor
semantics; the [API reference](reference.md) shows in-memory bundle compilation.
