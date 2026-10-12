# Studio instruments

**Validation status:** the library meets its numerical acceptance:
full-range renders, brighter sound at higher velocity, and calibrated levels.
The `electric_guitar` and `piano` added in 1.1.0 were also approved by ear
in listening rounds with the owner. The 1.0.0 exports were not: the owner
chose not to act as the listening gate then. The
[delivery report](reports/studio-delivery.md) records the measurements and their
limits.

`std/studio/1.1.0` is a built-in collection of an electric piano, an
acoustic piano, an electric guitar, a bass, a pad and a drum kit.
`std/studio/1.0.0` has the same exports without the guitar and the piano.
The library answers the T2 trial's finding that the std/basic
keys, bass, drums and pad sound dated ([F14](reports/ai-production-trial.md)). Like
the other built-in libraries, it needs no recordings, downloads, library
directory or network access.

The difference from std/basic is in the building blocks, not the presets.
These instruments use the [instrument palette](design/instrument-palette-proposal.md)
processors:
- velocity and key sources, so how hard and how high a note is played changes
  its sound as well as its level;
- curved envelopes, so struck sounds decay exponentially;
- the resonant `synth.svf/1` filter;
- the anti-aliased `synth.drive/1` saturation;
- in 1.1.0, the plucked string's pick hardness, pick position and pickup,
  the drive's level compensation, and the `synth.modal/1` resonator bank
  ([electric guitar and piano proposal](design/electric-guitar-piano-proposal.md)).

## Exports

| Export | Sound | What velocity does |
| --- | --- | --- |
| `electric_piano` | Tine electric piano: a pitch-proportional FM body and a short inharmonic tine through an asymmetric pickup drive, with stereo tremolo | Harder notes bark and open the filter; higher notes decay sooner |
| `bass` | Finger bass: a sine and a saw body with a plucked-string attack, through a low-pass that opens at each pluck | Harder notes open the filter further and damp the string less |
| `pad` | Four detuned saws in a stereo pair and a sub triangle, through a resonant low-pass that drifts slowly | Velocity opens the filter |
| `electric_guitar` (1.1.0) | A plucked string with a pick and a pickup, into one amp for all its strings: a level-compensated drive and a speaker cabinet | Harder picks are louder and brighter and drive the amp harder; higher notes decay sooner |
| `piano` (1.1.0) | A felt hammer striking two banks of stretched, decaying modes, long-ringing detuned strings and a short prompt sound, with a soundboard thump; stereo by key | Harder notes are louder, shorter and brighter strikes; the key sets stretch, decay and level |
| `drums` | A kit of the eight drums below, with the std/basic/1.1.0 kit's keys, gates and hat choke | Every drum gets brighter; kick and toms drop further |
| `kick`, `snare`, `clap`, `closed_hat`, `open_hat`, `low_tom`, `high_tom`, `crash` | The kit's pieces, also usable alone | As above |

```sh
maac instruments --library std/studio/1.1.0
maac instruments electric_piano --library std/studio/1.1.0 --json
```

Discovery gives each export's exact controls, ranges, pitch and duration
guidance and a runnable example.

## Switching from std/basic

Import the library and point a node at the new export. The drum keys are the
std/basic/1.1.0 kit's keys, so drum patterns need no change:

```maac
import studio { builtin = "std/studio/1.1.0"; }
node keys { instrument = &studio.electric_piano; config = { voices = 24; }; }
node bass { instrument = &studio.bass; config = { voices = 6; }; }
node pad { instrument = &studio.pad; config = { voices = 8; }; }
node drums { instrument = &studio.drums; }
```

Write velocities that mean something. A std/basic part written at one
velocity sounds the same on std/studio as a part played at one strength;
varying velocity across a part is what makes these instruments sound played.

## Levels

Every default `level` is calibrated on the same scale ([F15](reports/ai-production-trial.md)):
a reference phrase played at velocity 0.7 measures −20 LUFS integrated, within
±1 LU.

| Export | Reference phrase | Measured |
| --- | --- | --- |
| `electric_piano` | "Late Window" comping, four bars | −20.0 LUFS |
| `bass` | "Late Window" bass line, four bars | −20.0 LUFS |
| `pad` | "Late Window" B-section chords, four bars | −20.0 LUFS |
| `drums` | "Late Window" boom-bap beat with snare, four bars | −20.6 LUFS |
| `electric_guitar` | "Morning Ferry" power-chord chorus, four bars | −20.0 LUFS |
| `piano` | "Morning Ferry" verse chords as ballad accompaniment, four bars | −20.0 LUFS |

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
| `electric_guitar` | `level`, `drive` (clean at 1, crunch at 4 to 6, lead at 12), `tone` (cabinet cutoff), `pickup` (bridge 1/8 to neck 0.3), `mute` (string damping), `pan`, `release`, `vibrato` |
| `piano` | `level`, `brightness` (hammer low-pass), `release`, `width` (pan per octave from middle C), `hammer` (strike length), `tuning_spread` (string detune in cents) |
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

The pad's and the piano's voices are already stereo, so they have no `pan`
control.

## Guitar and piano

- **The guitar's amp is shared.** All the notes of one node go through one
  drive, so a power chord's strings distort together, as into a real amp.
  Give the rhythm and lead parts separate nodes.
- **Automate `drive` alone.** The drive holds its loudness: on the reference
  phrase, `drive` from 1 to 16 stays within 1 LU. A verse-to-chorus push
  needs no level automation beside it.
- **Lead settings.** The default bridge pickup suits rhythm. For a lead, set
  `pickup = 0.3` and `tone = 3200Hz`, the neck pickup's warmer sound, with
  `drive` near 12 and `vibrato` around 0.5.
- **The piano has no sustain pedal.** Every key is damped when its note ends,
  including the top octaves. Hold notes for longer to let them ring.
- **The piano reads sharp in the treble.** Its stretched partials make notes
  read up to about 20 cents sharp in `maac analyze`, rising towards the
  treble, as a stretch-tuned piano does. `pitch_mismatch` is not raised.

## Rendering cost

These graphs do more per voice than std/basic. Every new filter and drive
node costs 2 work units per sample, a `synth.modal/1` bank one unit for every
four resonators, and every other node 1. A piano voice is the most expensive:
a three-minute ballad part costs about 2.7 billion of the song profile's
10 billion units. Most of the time
goes to the engine's per-node overhead, not the DSP. Long arrangements may
need `--profile song`. The [delivery report](reports/studio-delivery.md) gives
measured render times.

## Versioning

The exact import identities are `std/studio/1.0.0` and `std/studio/1.1.0`,
recorded as `@builtin/std/studio/<version>.maac` with their source hashes.
`1.1.0` is `1.0.0` with the guitar and piano appended; every 1.0.0 export
renders bit-identically in both. The released bytes and sound are frozen; any
change to a timbre, default or level needs a new version.
