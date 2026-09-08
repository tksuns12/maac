# Soldier of Fortune

Edit [`main.maac`](main.maac). It is the complete, authoritative composition;
rendering needs this file and the two built-in libraries named in its imports.
The MIDI inspection files, one-time generator, manifests, and old rendering stems
are not runtime dependencies.

The opening portion shows the project, tempo and meter, instrument settings,
stereo master routing, all tracks and placements, and automation bindings.
Detailed note patterns and controller curves follow. The 14 original MIDI musical
tracks map to 24 native tracks because each MaaC track has one instrument target;
the drum kit therefore has separate lanes. Repeated rendering splits have been
merged back into their musical parts.

Change the `gain` parameter on `node master` for overall volume. Individual
instrument `level`, `brightness`, `release`, and `pan` settings are nearby.
Controller curves may override initial settings later in the score; edit their
bindings/points when changing automated behavior. The master gain is applied
once after the stereo ensemble sum.

Build the complete song with the native command:

```sh
cd compositions/soldier-of-fortune
maac build --profile song -o soldier-of-fortune.wav --format pcm16
```

Omitting the input selects `main.maac` in the current directory. From the
repository root, a directory input works too:

```sh
maac build compositions/soldier-of-fortune --profile song \
  -o soldier-of-fortune.wav --format pcm16
```

The composition requires 4,165,619,697 instrument-work units, exceeding the
default 500 million limit. The explicit `song` profile permits up to 10 billion;
it is required again when rendering a retained plan. This is one composition
and one native render, without a project-specific Python splitter. Output is
48 kHz stereo PCM16, 9,620,109 frames (about 200.419 seconds).

The source preserves all 2,399 supplied MIDI notes, their stable numeric IDs,
pitches, note gates and onset/release velocities, all 17 tempo points, 4/4 meter,
the 424-quarter-note score, and the two-second tail. Normalized MIDI CC7,
pan, pitch bend, and modulation behavior match the verified acoustic rendition.
Instrument and percussion substitutions remain explicit in the lane declarations;
channel pressure remains unmapped. No verse/chorus labels were inferred.

Source supplied by the user: <https://bitmidi.com/uploads/38560.mid>.
MIDI SHA-256: `6d54d5b1b8eb3c3fea7428eaf76db93bb52624f3a51ed1c66fa5f745d254f974`.
The versioned libraries are `std/acoustic/1.0.0` and `std/basic/1.0.0`.

Verified with a freshly installed MaaC executable copied outside the checkout:
omitted-input check, compile, and full build succeeded with only `main.maac` as
the initial input file. After removing that source copy, rendering its retained
plan with `--profile song` produced a byte-identical WAV. Omitting the profile
correctly rejected the work budget and preserved an existing output file.

The native plan audit matched all notes, tempos, instrument controls, and
automation against the verified acoustic rendition. The output peak is
0.849971 with no clipping. Compared with the previous WAV assembled from
Float32 stems, only 1,312 of 19,240,218 samples differ, each by one PCM16 count
(RMS difference 0.008258 counts). These measurements establish numerical
fidelity; they do not claim a listening comparison.

Local acceptance artifacts are under `target/soldier-of-fortune-main/`:
`soldier-of-fortune.wav`, `soldier-of-fortune.performance.json`,
`native-verification.json`, `standalone-plan-audit.json`, and
`standalone-commands.json`. They are outputs and verification evidence, not
inputs required to edit or render this composition.
