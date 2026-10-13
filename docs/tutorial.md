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

The graph is explicit. The sine produces mono audio, and `core.pan/1` converts it
to stereo using equal-power panning. For several instruments, connect their
stereo pan outputs to a `core.sum/1` node configured with `channels = 2`, then
name that node's `out` as the project output. There is no implicit track mixer.

For a worked nested pattern and filter automation, read
[`example.maac`](../spec/example.maac). For a complete sixteen-bar composition,
read [`evening-window.maac`](../examples/evening-window.maac). Keep output filenames
new, or add `--force` when deliberately replacing a previous render.
