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

The graph is explicit. The sine produces mono audio, and `core.pan/1` converts it
to stereo using equal-power panning. For several instruments, connect their
stereo pan outputs to a `core.sum/1` node configured with `channels = 2`, then
name that node's `out` as the project output. There is no implicit track mixer.

For a worked nested pattern and filter automation, read
[`example.maac`](../example.maac). For a complete sixteen-bar composition,
read [`evening-window.maac`](../evening-window.maac). Keep output filenames
new, or add `--force` when deliberately replacing a previous render.
