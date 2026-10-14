# Authoring a first score

Save this complete source as `first.maac`:

```maac
maac 1;

project first {
  score = [0q, 4q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &position:out;
  tail = 1s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }

pattern phrase {
  length = 4q;
  note a { at = 0q; dur = 1q; pitch = C4; velocity = 0.7; }
  note b { at = 1q; dur = 1q; pitch = E4; velocity = 0.6; }
  note c { at = 2q; dur = 2q; pitch = G4; velocity = 0.7; }
}
track melody { target = &instrument:events; }
place main { pattern = &phrase; track = &melody; at = 0q; }
node instrument {
  type = "core.sine/1";
  config = { voices = 8; };
  params = { attack = 5ms; release = 80ms; level = 0.2; };
}
node position { type = "core.pan/1"; params = { pan = 0; }; }
connect melody_position { from = &instrument:out; to = &position:in; }
```

Build and inspect it:

```sh
maac check first.maac
maac compile first.maac -o first.performance.json
maac render first.performance.json -o first.wav
```

One `q` always means one quarter note. At 120 bpm it lasts half a second. The
four-quarter score above lasts two seconds, followed by its explicit one-second
tail. A note's `dur` is its gate; the sine instrument's release envelope continues
after that gate ends. Rests need no objects: leave the desired interval empty.

To repeat the phrase twice, change the project score to `[0q, 8q]` and add
`count = 2;` to `place main`. A placement's occupied repetitions must fit in the
score. Repetition does not change a source note's identity: the first note has
addresses `main/0/a` and `main/1/a` in the two repetitions.

To make the phrase last twice as long, use `stretch = 2;` and extend the score
accordingly. Stretch changes musical times and durations. Physical envelope
parameters and note offsets remain in seconds. `transpose = 1200ct;` raises a
placement by one octave; `transpose = -1200ct;` lowers it by one octave.

To delay only the second repetition's first note by 20 milliseconds, put this
child inside the twice-repeated placement:

```maac
override later_a {
  event = "1/a";
  set = { onset_offset = 20ms; release_offset = 20ms; };
}
```

Setting both offsets preserves its physical gate length. Setting only the onset
offset shortens it. Overrides apply after inherited transforms. A replacement
pitch is the final pitch, and replacement `at` is relative to the placement's
origin. Other repetitions retain the source note.

Use `boundary = cut;` when a repeated note must release at its repetition
boundary. The default `spill` allows its gate to extend beyond that boundary.
Neither boundary removes the instrument's release envelope. Project score end
always truncates held gates; the tail permits their envelopes to finish.

A `chord` leaf writes notes that share an onset and a gate. For a strummed
guitar, name each chord shape once as a `voicing` and write the strum rhythm
once:

```maac
voicing g { pitches = [G2, B2, D3, G3, B3, G4]; }
voicing d { pitches = [D3, A3, D4, F#4]; }
pattern strum_bar {
  length = 4q;
  chord down { at = 0q; dur = 1q; strum = down; velocity = 0.78; tilt = -0.12; }
  chord up { at = 3/2q; dur = 1q; top = 4; strum = up; velocity = 0.46; }
  chord ring { at = 2q; dur = 2q; strum = down; stagger = 30ms; velocity = 0.7; }
}
pattern progression {
  length = 8q;
  use first { pattern = &strum_bar; at = 0q; voicing = &g; }
  use second { pattern = &strum_bar; at = 4q; voicing = &d; }
}
```

The chords in `strum_bar` have no pitches of their own, so each plays the
voicing its `use` names. `strum = down` strikes the strings from the first
pitch to the last, 8 ms apart unless `stagger` says otherwise, and
`strum = up` strikes them in reverse. `tilt` changes the velocity from the
first string struck to the last: `down` strikes G2 at 0.78 and G4 at 0.66.
`top = 4` keeps the upper four strings, the ones an upstroke catches. A chord
can also name its shape itself, with `voicing = &g` or
`pitches = [C4, E4, G4]`. Each string is a member with its own address:
`down.0` to `down.5` under `g`, and `down.0` to `down.3` under `d`.

Several parts can play one progression. Write the chords once as a
`harmony`, give each part its voicing for every chord as a `voicings`, and
let a placement supply both to a rhythm of chords without pitches:

```maac
harmony verse_chords { length = 8q; chords = [(0q, "G"), (4q, "D")]; }
voicings guitar_voicings { chords = [("G", &g), ("D", &d)]; }
place strums { pattern = &strum_bar; track = &guitar_notes; at = 0q; count = 4; harmony = &verse_chords; voicings = &guitar_voicings; }
```

Each chord in `strum_bar` plays the guitar's voicing for the chord sounding
at its position: the bars play G, D, G, D, because a harmony repeats every
`length`. Chord names are labels, compared exactly. Another part, such as a
piano, places its own rhythm with the same `harmony` and its own
`voicings`, so a chord change is one edit. A bass can follow with voicings
of each root and its octave, played with `bottom = 1` and `top = 1`.

`pick` plays chosen members by their position in the voicing, counted from
1, so an arpeggio can follow the harmony too:

```maac
pattern arp {
  length = 2q;
  chord a1 { at = 0q; dur = 1/2q; pick = 1; }
  chord a2 { at = 1/2q; dur = 1/2q; pick = 2; }
  chord a3 { at = 1q; dur = 1/2q; pick = 3; }
  chord a4 { at = 3/2q; dur = 1/2q; pick = [2, 4]; }
}
```

Under G, `a3` plays D3 and `a4` plays B2 and G3; under D, they play D4, then
A3 and F#4. Positions in a list strictly increase. A position beyond the
voicing is an error that names the chord and the voicing, so every voicing
the arpeggio plays needs as many members as its highest pick.

A note can bend and add vibrato through a pitch expression. Name the vibrato
once and give each held note the expression:

```maac
vibrato guitar { rate = 11/2Hz; depth = 38ct; delay = 300ms; rise = 450ms; shape = up; }
curve bend_up {
  clock = seconds;
  points = [(0ms, -200ct, step), (40ms, -200ct, linear), (170ms, 0ct, step)];
}
pattern lead {
  length = 4q;
  note held { at = 0q; dur = 2q; pitch = A4; expression vib { kind = pitch; vibrato = &guitar; } }
  note peak { at = 2q; dur = 2q; pitch = E5; expression bend { kind = pitch; curve = &bend_up; vibrato = &guitar; } }
}
```

`held` sounds A4 for 300 ms, then swings up to 38 cents above it and back,
5.5 times a second, the swing growing over 450 ms. `shape = up` rises from
the pitch, as a guitarist's vibrato does; `down` falls from it, and the
default `centered` swings half the depth either side. `peak` is picked a
whole step low and bent up to E5 within 170 ms, then takes the same vibrato.
A vibrato starts once its curve has ended. The compiler writes both out as
the note's pitch curve, up to the end of its gate. For another waveform,
draw one cycle as a `normalized` curve and name it as the `shape`: a square
cycle with `depth = 200ct` is a whole-step trill.

The graph is explicit. The sine produces mono audio, and `core.pan/1` converts it
to stereo using equal-power panning. For several instruments, connect their
stereo pan outputs to a `core.sum/1` node configured with `channels = 2`, then
name that node's `out` as the project output. There is no implicit track mixer.

For a worked nested pattern and filter automation, read
[`example.maac`](../spec/example.maac). For a complete sixteen-bar composition,
read [`evening-window.maac`](../examples/evening-window.maac). Keep output filenames
new, or add `--force` when deliberately replacing a previous render.
