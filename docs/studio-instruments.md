# Studio instruments

**Validation status:** measured, not listened to. The library meets its
numerical acceptance: full-range renders, brighter sound at higher velocity,
and calibrated levels. No one has approved its sound by ear; the owner chose
not to act as the listening gate. The [delivery report](studio-delivery.md)
records the measurements and their limits.

`std/studio/1.0.0` is a built-in collection of an electric piano, a bass, a
pad and a drum kit. It answers the T2 trial's finding that the std/basic
keys, bass, drums and pad sound dated ([F14](ai-production-trial.md)). Like
the other built-in libraries, it needs no recordings, downloads, library
directory or network access.

The difference from std/basic is in the building blocks, not the presets.
These instruments use the [instrument palette](instrument-palette-proposal.md)
processors:
- velocity and key sources, so how hard and how high a note is played changes
  its sound as well as its level;
- curved envelopes, so struck sounds decay exponentially;
- the resonant `synth.svf/1` filter;
- the anti-aliased `synth.drive/1` saturation.

## Exports

| Export | Sound | What velocity does |
| --- | --- | --- |
| `electric_piano` | Tine electric piano: a pitch-proportional FM body and a short inharmonic tine through an asymmetric pickup drive, with stereo tremolo | Harder notes bark and open the filter; higher notes decay sooner |
| `bass` | Finger bass: a sine and a saw body with a plucked-string attack, through a low-pass that opens at each pluck | Harder notes open the filter further and damp the string less |
| `pad` | Four detuned saws in a stereo pair and a sub triangle, through a resonant low-pass that drifts slowly | Velocity opens the filter |
| `drums` | A kit of the eight drums below, with the std/basic/1.1.0 kit's keys, gates and hat choke | Every drum gets brighter; kick and toms drop further |
| `kick`, `snare`, `clap`, `closed_hat`, `open_hat`, `low_tom`, `high_tom`, `crash` | The kit's pieces, also usable alone | As above |

```sh
maac instruments --library std/studio/1.0.0
maac instruments electric_piano --library std/studio/1.0.0 --json
```

Discovery gives each export's exact controls, ranges, pitch and duration
guidance and a runnable example.

## Switching from std/basic

Import the library and point a node at the new export. The drum keys are the
std/basic/1.1.0 kit's keys, so drum patterns need no change:

```maac
import studio { builtin = "std/studio/1.0.0"; }
node keys { instrument = &studio.electric_piano; config = { voices = 24; }; }
node bass { instrument = &studio.bass; config = { voices = 6; }; }
node pad { instrument = &studio.pad; config = { voices = 8; }; }
node drums { instrument = &studio.drums; }
```

Write velocities that mean something. A std/basic part written at one
velocity sounds the same on std/studio as a part played at one strength;
varying velocity across a part is what makes these instruments sound played.

## Levels

Every default `level` is calibrated on the same scale ([F15](ai-production-trial.md)):
a reference phrase played at velocity 0.7 measures −20 LUFS integrated, within
±1 LU.

| Export | Reference phrase | Measured |
| --- | --- | --- |
| `electric_piano` | "Late Window" comping, four bars | −20.0 LUFS |
| `bass` | "Late Window" bass line, four bars | −20.0 LUFS |
| `pad` | "Late Window" B-section chords, four bars | −20.0 LUFS |
| `drums` | "Late Window" boom-bap beat with snare, four bars | −20.6 LUFS |

Within the kit, the pieces are balanced for boom-bap. Measured alone on one
hit per beat, the kick and snare are equal, and relative to them the clap is
−2 LU, the toms −1 LU, the crash −6 LU, the open hat −8 LU and the closed hat
−10 LU.

The scale covers std/studio only; std/basic and std/acoustic keep their
released defaults. When mixing across libraries, measure with `maac analyze`.

## Controls

| Export | Controls |
| --- | --- |
| `electric_piano` | `level`, `brightness` (filter cutoff before key tracking and velocity), `release`, `pan`, `bark` (pickup drive before velocity), `tremolo_depth`, `tremolo_rate` |
| `bass` | `level`, `brightness`, `release`, `pan`, `growl` (drive) |
| `pad` | `level`, `brightness`, `attack`, `release`, `motion` (filter drift depth) |
| Each drum | `level`, `brightness`, `pan` |
| `drums` | `<piece>_level`, `<piece>_pan`, `<piece>_brightness` for each piece |

Values that would leave a parameter's range fail without clamping. Two
interactions to watch:
- The electric piano's tremolo moves `pan`, so keep `|pan| + tremolo_depth`
  at or below 1. The default depth is 0.2; set it to 0 for a still image.
- The electric piano's low-pass tracks six times the note frequency plus
  `brightness` and up to 2.5 kHz from velocity. With the defaults, notes from
  A7 up at full velocity exceed the filter's range; the catalog range stops
  at C7.

The pad's voice is already stereo, so it has no `pan` control.

## Rendering cost

These graphs do more per voice than std/basic. Every new filter and drive
node costs 2 work units per sample and every other node 1. Most of the time
goes to the engine's per-node overhead, not the DSP. Long arrangements may
need `--profile song`. The [delivery report](studio-delivery.md) gives
measured render times.

## Versioning

The exact import identity is `std/studio/1.0.0`, recorded as
`@builtin/std/studio/1.0.0.maac` with its source hash. The released bytes and
sound are frozen; any change to a timbre, default or level needs a new
version.
