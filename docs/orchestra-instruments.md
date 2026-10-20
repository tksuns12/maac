# Orchestra instruments

**Validation status:** the library meets its numerical acceptance:
full-range renders, brighter sound at higher velocity, calibrated levels and
connected lines. Its strings reproduce the bowed-string prototype that the
owner accepted in the [T6 trial](reports/ai-production-trial-t6.md); see the
[bowed strings proposal](design/bowed-strings-proposal.md#results).

`std/orchestra/1.0.0` is a built-in collection of orchestral strings and a
concert harp. Like the other built-in libraries, it needs no recordings,
downloads, library directory or network access.

The strings are sections of [bowed strings](instruments.md#bowed-strings):
every player is a bowed waveguide string with a wooden body, its own tuning,
vibrato and timing. Notes that touch are played
[connected](../MaaC-1-Specification.md#83-connected-notes), on one string
with one bow.

## Exports

| Export | Sound | Range |
| --- | --- | --- |
| `violins` | Eight players on violin bodies, left of centre | G3–B6 |
| `violas` | Three players to a line on viola bodies, with a narrower vibrato; a three-note chord is a divided section of nine | C3–E6 |
| `cellos` | Five players on cello bodies, bowed a little nearer the bridge | C2–A5 |
| `basses` | Three players on bass bodies, with a slow, narrow vibrato | E1–G4 |
| `concert_harp` | A fingertip pulse into 24 unstretched string modes plucked near the middle, with a faint snap; notes ring for seconds | C2–G7 |

## Playing the strings

```maac
import orchestra { builtin = "std/orchestra/1.0.0"; }
node violins { instrument = &orchestra.violins; config = { voices = 4; }; }
track melody { target = &violins:events; }
pattern line {
  length = 8q;
  note a { at = 0q; dur = 2q; pitch = D5; velocity = 0.7; }
  note b { at = 2q; dur = 2q; pitch = A5; velocity = 0.8; slide = 200ms; }
  note c { at = 4q; dur = 3/2q; pitch = Bb5; velocity = 0.75; }
  note d { at = 11/2q; dur = 1/2q; pitch = A5; velocity = 0.7; slur = true; }
}
```

- **Lines.** A note that starts where the note before it on the track ends
  continues the string; the bow changes direction, about 65 ms before the
  beat, so the new bow speaks on it. `slur = true` keeps the bow and only
  the finger moves. A rest lifts the bow, and the next note starts a new
  stroke. A line needs one voice, plus a few for the strokes' tails.
- **Chords** join member by member: a pad of three-note chords on the
  violas is three lines. Bowing two bars to a stroke, with the second
  chord slurred, sounds like a section holding a pad.
- **Slides.** `slide` slides into a note, slow to leave and quick to land.
  A slide into a bow change finishes on the old bow. Slides on the leaps
  and the falls, 80–200 ms, gave the T6 violins their character.
- **Dynamics.** Velocity sets 3/5 of the bow's `dynamics` and pressure
  2/5, so a pressure curve is a swell and a crescendo. Speed and force
  rise together, so the tone stays nearly the same and the level follows:
  across velocities 0.3 to 1 the spectrum's centre moves by under a sixth
  of an octave. Velocity also scales the level, as on every instrument.
- **Short strokes.** A short, accented note after a rest needs a short
  `attack`, such as 15 ms; give it a node of its own with a short `release`.
- **Range.** The violins are in tune up to B6; above that this string model
  can leave its normal motion. A soft bow on the lowest cello and bass
  notes wavers by a few cents in its first second, and at the loudest
  dynamics a few of the lowest bass notes can jump an octave for part of a
  stroke.

## Levels

Each export's default level puts four bars of its reference phrase at
velocity 0.7 at −20 LUFS: a slurred violin melody, held three-note viola
chords, whole notes on the cellos and the basses, and harp arpeggios in
eighths. The violins stand left of centre and the low strings right;
`pan` and `width` move and spread a section.

## Controls

| Control | Strings | Harp |
| --- | --- | --- |
| `level` | Section level | Level |
| `attack` | Time for a stroke after a rest to reach full speed | — |
| `release` | Amplitude release after the bow lifts | — |
| `vibrato`, `rate` | Vibrato depth in cents and rate | — |
| `position` | Bow distance from the bridge, 1/10 to 1/7 | — |
| `timing` | Spread of the players' bow changes | — |
| `pan`, `width` | Centre and stereo spread of the section | `pan` only |
| `low_cut` | High-pass in the stage | — |

## Rendering cost

Each player costs 33 units of work per sample, with its body: a violin
section of eight is 264 units, about as much as 264 sine voices. "Homeward",
with 25 string players in its widest passage, renders its 128 seconds in
under four minutes of CPU, within the `song` profile.

## Versioning

The source bytes of `std/orchestra/1.0.0` are frozen
([`stdlib/orchestra/1.0.0.sha256`](../stdlib/orchestra/1.0.0.sha256)). A
change to a sound, a default or a seed needs a new version.
