# Studio instruments delivery report

**Result:** `std/studio/1.0.0` meets its numerical acceptance and is frozen.
No one has approved its sound by ear: after one audition round the owner
declined to act as the listening gate ([decision 7](instrument-palette-proposal.md#decisions)).
These measurements show that the instruments behave as designed. They do not
show that they sound good.

The [guide](studio-instruments.md) explains how to use the library. This
report records how it was tuned and what was measured. All renders are
48 kHz from the release build on an Apple M1, 2026-10-06.

## Acceptance

| Criterion | Evidence | Result |
| --- | --- | --- |
| Every pitched export renders its catalog range | Every semitone of each catalog range (electric piano C1–C7, bass E1–G3, pad C2–C6) at velocities 0.2, 0.5 and 1 | Finite, peak below full scale |
| Every drum renders at its tested pitches | C-1, C2, C4 and G9 at velocity 1 | No clipped samples |
| Harder is brighter | One note or hit at velocity 0.3 and 1; band-weighted mean octave from `maac analyze` | Higher at velocity 1 for all 3 pitched exports and all 8 drums |
| Levels are calibrated | Reference phrases at velocity 0.7 | Within ±1 LU of −20 LUFS, true peak below −1 dBTP |
| Drums keep headroom | Each piece alone at velocity 1, at its default level | True peak below −1 dBTP |
| The kit matches std/basic | Keys, gates and choke groups compared with the std/basic/1.1.0 kit | Identical |
| The bytes are frozen | Source SHA-256 against `stdlib/studio/1.0.0.sha256` | Matches |

[`tests/studio_instruments.rs`](../tests/studio_instruments.rs) checks every
row.

## Velocity response

std/basic gives the same spectrum at every velocity: in the same renders, its
electric piano's spectral centroid stays at 203–207 Hz on C3 from velocity
0.25 to 1. The std/studio figures below are spectral centroids over the first
100 ms after the attack.

| Electric piano | v0.25 | v0.5 | v0.8 | v1 |
| --- | ---: | ---: | ---: | ---: |
| C3 | 170 Hz | 197 Hz | 242 Hz | 281 Hz |
| C4 | 326 Hz | 397 Hz | 485 Hz | 559 Hz |
| C5 | 647 Hz | 780 Hz | 950 Hz | 1,091 Hz |

At velocity 1 the second harmonic is 6 dB under the fundamental and the third
to fifth are 19–23 dB under it: the bark. At velocity 0.25 they are 16 dB and
28–49 dB under.

| Bass | v0.25 | v0.5 | v0.8 | v1 |
| --- | ---: | ---: | ---: | ---: |
| E1 | 182 Hz | 232 Hz | 293 Hz | 334 Hz |
| E2 | 248 Hz | 295 Hz | 350 Hz | 387 Hz |
| A2 | 313 Hz | 372 Hz | 440 Hz | 484 Hz |

The second to fourth harmonics sit 4–14 dB under the fundamental, the range
of a finger bass.

| Drum | Centroid at v0.35 | Centroid at v1 | −40 dB after |
| --- | ---: | ---: | ---: |
| Kick | 108 Hz | 142 Hz | 281–294 ms |
| Snare | 4,651 Hz | 6,114 Hz | 124–150 ms |
| Clap | 4,073 Hz | 4,505 Hz | 109–111 ms |
| Closed hat | 9,404 Hz | 9,989 Hz | 37 ms |
| Open hat | 8,943 Hz | 9,482 Hz | 246–249 ms |
| Low tom | 123 Hz | 129 Hz | 315–329 ms |
| High tom | 182 Hz | 187 Hz | 268 ms |
| Crash | 12,690 Hz | 13,035 Hz | over 600 ms |

The toms change least: velocity deepens their pitch drop rather than adding
noise.

The pad is stereo in the voice: in the B-section audition its channels
correlate at 0.72, with the side signal 7.8 dB under the mid. The std/basic
pad is mono.

## Levels

| Reference phrase at velocity 0.7 | Integrated | True peak |
| --- | ---: | ---: |
| Electric piano, "Late Window" comping | −20.00 LUFS | −7.6 dBTP |
| Bass, "Late Window" line | −20.02 LUFS | −11.4 dBTP |
| Pad, "Late Window" B chords | −20.00 LUFS | −9.3 dBTP |
| Drums, "Late Window" beat with snare | −20.62 LUFS | −3.1 dBTP |

Each piece alone, one hit per beat at velocity 0.7, relative to the kick:
snare 0 LU, clap −2.0, low and high tom −1.0, crash −6.0, open hat −8.0,
closed hat −10.0. At velocity 1 the loudest single hit, the clap, peaks at
−2.9 dBFS.

## Tuning record

The first draft was measured and changed before freezing:

- **Electric piano bark.** At velocity 1 the second harmonic was 1 dB above
  the fundamental and the fifth and sixth 16–18 dB under it. The pickup drive
  and FM depth now respond less to velocity: the second harmonic is 6 dB under.
- **Bass.** The plucked string's white-noise excitation spread its energy
  over hundreds of harmonics, so the attack hissed and the low harmonics were
  weak: centroids were 740–1,380 Hz, against 79–181 Hz for std/basic. A sine
  and a saw now carry the body, a fast envelope opens the filter at each
  pluck, and the string only adds the attack.
- **Drum tails.** The kit's 100–350 ms gates cut the drums after a 60 ms
  release: the kick fell 40 dB within 125 ms. Releases now continue each
  decay, except the hats, whose short release keeps the choke.
- **Crash.** Velocity lowered its high-pass, so harder hits were darker. It
  now raises it.
- **Hat peaks.** The six square waves all started at phase 0, adding up to a
  spike at every hit. Spreading their start phases lowered the closed hat's
  true peak by 9 dB while its loudness fell by 1 dB.
- **Snare peaks.** Calibrated to −20 LUFS, a full-velocity snare peaked at
  +1.4 dBFS. A drive in its shared graph, after velocity, now soft-clips loud
  hits: the same hit peaks at −5.0 dBFS.

## Render cost

CPU cycles for `maac build` on an Apple M1, from `time -l`. They do not
depend on other load on the machine. At the M1's full clock, 3.2 billion
cycles are about one second.

| Example | Audio | Cycles at release | Cycles after the engine work |
| --- | ---: | ---: | ---: |
| [`electric_piano.maac`](../examples/studio/electric_piano.maac) | 12.2 s | 5.72 billion | 3.28 billion |
| [`pad.maac`](../examples/studio/pad.maac) | 12.7 s | 7.13 billion | 4.57 billion |
| [`drums.maac`](../examples/studio/drums.maac) | 18.0 s | 9.05 billion | 3.77 billion |
| [`bass.maac`](../examples/studio/bass.maac) | 11.2 s | 2.50 billion | 1.23 billion |

Every example renders in well under real time: the electric piano took about
1.8 s of CPU for 12.2 s of audio at release, and about 1.0 s after the
[render-speed work](render-speed.md). An earlier version of this table
reported wall-clock times taken while a test suite compiled in parallel; they
overstated the cost several times over and have been replaced.

## Limits

- **Not listened to.** Each number above measures one property. Together
  they do not establish that an instrument sounds convincing.
- **The clap's second burst swells.** An envelope cannot start late, so the
  second burst rises over 10 ms instead of starting sharply.
- **The hats are drum-machine metal.** Six square waves through filters, as
  analog machines made them, not a recorded cymbal.
- **The drive still aliases at the top.** Hard-driven high electric-piano
  notes keep some aliasing near 24 kHz ([saturation](instruments.md#saturation)).
- **The electric piano's filter range.** Notes from A7 up at full velocity
  exceed the filter's range; the catalog stops at C7.

## Reproducing

```sh
cargo build --release
python3 scripts/studio_auditions.py              # A/B files in target/studio-auditions
python3 scripts/studio_auditions.py --calibrate  # reference loudness
cargo test --release --test studio_instruments
```

Each `*-AB.wav` plays std/basic, two seconds of silence, then std/studio, both
scaled to −20 LUFS. The std/basic side uses the settings "Late Window" used.
