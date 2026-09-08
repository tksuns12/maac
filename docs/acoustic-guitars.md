# MaaC-native acoustic guitars

**Validation status:** kernel, instrument range/control, and fresh installed
authoring checks passed on the tested host. User listening approval remains
pending. The [delivery report](acoustic-delivery.md) records exact evidence and
its limits.

`std/acoustic/1.0.0` is a separate collection of three stereo instruments:
`nylon_guitar`, `steel_guitar`, and `muted_guitar`. Each uses MaaC's internally
recirculating `synth.pluck/1` string and existing filters for body weighting.
Their changing partial decay comes from stored string history. They require
no recordings, downloaded samples, sound-library directory, or network access.

These are synthesized string models with body EQ. They do not simulate a
complete guitar body, pick position, excitation hardness, or velocity-dependent
timbre. Velocity scales note amplitude. Numerical measurements and a user's
judgment that a sound is more convincingly acoustic are separate evidence.

The existing 24 [basic instruments](basic-instruments.md) retain their frozen
graphs and defaults. The same guitar names occur in both collections; the
explicit library identity determines which sound you use.

## Select and inspect a guitar

```sh
maac instruments --libraries
maac instruments --library std/acoustic/1.0.0
maac instruments nylon_guitar --library std/acoustic/1.0.0 --json
```

Omitting `--library` keeps the existing `std/basic/1.0.0` behavior.
`--libraries` cannot be combined with a name or `--library`. Unknown library
versions and names fail explicitly; there is no latest-version alias or
fallback. Detailed discovery includes the exact control metadata, musical
guidance, and a complete source example.

JSON uses `catalog` for an instrument list and `instrument` for named detail.
Explicit selection also supplies top-level `library`; `--libraries` supplies
`libraries`, each with `library`, `source_path`, and `source_hash`. Existing
default command results keep their shape. Discovery obtains hashes from actual
embedded bytes, so there is no hand-copied hash to maintain in this guide.

## First composition

Save this complete composition as `guitar.maac`:

```maac
maac 1;
project song {
  score = [0q, 4q]; rate = 48000Hz;
  tempo = &clock; meter = &metre; output = &guitar:out; tail = 1s;
}
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
import acoustic { builtin = "std/acoustic/1.0.0"; }
node guitar {
  instrument = &acoustic.nylon_guitar;
  config = { voices = 4; };
}
pattern phrase {
  length = 4q;
  note c { at = 0q; dur = 1q; pitch = C4; velocity = 0.6; }
  note e { at = 1q; dur = 1q; pitch = E4; velocity = 0.5; }
  note g { at = 2q; dur = 3/2q; pitch = G4; velocity = 0.6; }
}
track notes { target = &guitar:events; }
place once { pattern = &phrase; track = &notes; at = 0q; }
```

```sh
maac check guitar.maac
maac compile guitar.maac -o guitar.performance.json
maac render guitar.performance.json -o guitar.wav --format pcm16
```

Select `&acoustic.steel_guitar` or `&acoustic.muted_guitar` in the instance to
audition the other guitars. Keep the same notes and velocity when comparing
timbres. `maac build guitar.maac -o guitar.wav --format pcm16` combines compile
and render. Existing output files require `--force`.

## Controls and live expression

Every acoustic export has six public controls. Discovery provides the exact
defaults and permitted ranges from its graph definition.

| Control | Meaning | Unit and timing |
| --- | --- | --- |
| `level` | Gain over the complete instrument output | Dimensionless; every sample |
| `brightness` | Output one-pole low-pass cutoff | Hz, strictly between 0 and 24000; every sample |
| `release` | Designated amplitude ADSR release after note-off | Seconds; captured at note-off |
| `pan` | Equal-power stereo position, −1 left through +1 right | Dimensionless; every sample |
| `bend_ratio` | Base live string frequency multiplier; default 1 | Dimensionless, 1/8…8; every sample |
| `vibrato_amount` | Gain of an internal 5 Hz sine LFO; default 0 | Dimensionless, 0…16; every sample |

The current authored graph adds `0.0145 * vibrato_amount * sin(phase)` to
`bend_ratio`. The effective string frequency is the note frequency multiplied
by that sum. Vibrato therefore changes the ratio additively; it is not a
cents-valued parameter. The 5 Hz LFO starts at phase zero independently for
each note. As an authoring starting point, use `vibrato_amount` between 0 and 1;
the inherited 0…16 range is a parameter bound, not a musical recommendation.
Likewise, ratios approximately 0.890899 through 1.122462 express a two-semitone
down/up bend. The complete modulated ratio must stay within 1/8…8, and its
effective frequency must remain within 20–4000 Hz.

For a smooth bend, add this curve and automation to the composition above:

```maac
curve bend {
  clock = score;
  points = [(0q, 1, linear), (1/2q, 1.122462, linear), (1q, 1, step)];
}
automation bend_guitar { target = &guitar.params.bend_ratio; curve = &bend; at = 2q; }
```

Set `params = { vibrato_amount = 0.3; };` on the guitar instance for a modest
periodic ratio variation. Frozen version 1.0.0 defaults are:

| Instrument | Level | Brightness | Release |
| --- | --- | --- | --- |
| `nylon_guitar` | 0.34 | 3500 Hz | 180 ms |
| `steel_guitar` | 0.28 | 6500 Hz | 240 ms |
| `muted_guitar` | 0.32 | 2200 Hz | 60 ms |

All three default to `pan=0`, `bend_ratio=1`, and `vibrato_amount=0`.

Changing the string's effective ratio changes its fractional delay while
retaining stored excitation. It does not retrigger the note. Global automation
applies to all active voices in that instrument instance; independent lead and
chord expression requires separate instances. There are no new per-note
expression lanes. Smooth curves can bend an existing note; abrupt parameter
steps can make artifacts because the processor adds no hidden smoothing.

`brightness` is a cutoff in Hz, not a normalized damping macro. String damping
and interpolation affect partial decay separately from output EQ. Their
combined response need not be monotonic. `release` determines note-off damping
and voice retirement; it does not replace the string's natural decay.

An instance defaults to 64 voices. Count overlapping gates and release tails
when choosing capacity. Silent or naturally decayed held strings retain their
voices until the designated release retires them. Overflow is an explicit
error, with no voice stealing. Zero level, velocity, or amplitude still advances
string history. A render reset restarts deterministic seed excitation.

The processor's effective frequency must remain within 20–4000 Hz after ratio
and vibrato/modulation. This is a hard DSP boundary, not a tested guitar range.
Invalid evaluated values fail without clamping, even when the output is muted.
All 49 semitones E2–E6 (MIDI 40–88) were rendered for each guitar. E2 and E6
also passed combined two-semitone down/up bends with full recommended vibrato
(`vibrato_amount=1`). Four-second held notes, overlapping chords, common
controls, smooth expression, replay, and retained plans were checked. Catalog
gate guidance is 0.08–6 seconds; this recommendation is distinct from the
specific gates exercised by the tests. These checks establish rendering
behavior, not a realism judgment.

The [three individual auditions](../examples/acoustic/) last 7.5 seconds each;
the [fingerpicked phrase](../examples/acoustic/fingerpicked_phrase.maac) lasts
5.7 seconds. Render them with the installed executable:

```sh
maac build examples/acoustic/nylon_guitar.maac -o nylon.wav --format pcm16
maac build examples/acoustic/steel_guitar.maac -o steel.wav --format pcm16
maac build examples/acoustic/muted_guitar.maac -o muted.wav --format pcm16
maac build examples/acoustic/fingerpicked_phrase.maac -o fingerpicked.wav --format pcm16
```

## Rendering, limits, and versions

Keep conservative output levels and moderate velocities when adding chords or
other instruments. PCM16 export rejects overload instead of normalizing or
limiting the signal. Float32 and PCM16 are 48 kHz. A project tail must be long
enough for the wanted release; it is the explicit final render boundary.

Each pluck voice-node uses 2402 f64 delay cells. The engine checks declared
capacity across a plan against 8,388,608 cells (64 MiB payload), separately
from existing voice and graph limits. Pluck sample visits cost 16 normalized
work units and each note initialization costs another 2402 units. These bounds
apply even to muted notes; see [resource limits](capabilities.md#resource-bounds).

The exact import identity is `std/acoustic/1.0.0`, recorded as
`@builtin/std/acoustic/1.0.0.maac` with its source hash. The reserved namespace
cannot be shadowed by local files. Embedded bytes, objects, and imports consume
the existing bundle budgets. Future changes to released timbres, seeds, or
defaults require a different library version; basic-library bytes are unchanged.

Compiled version 2 plans retain graphs, resolved seeds, and source/dependency
provenance. They do not require the original composition, a library directory,
or a current catalog when rendered. A stored hash is provenance, not registry
authentication of an edited plan. Source grammar and core/plan versions are
unchanged. The [implementation contract](plucked-string.md) fixes the precise
recurrence and acceptance criteria.

## Listening status

Auditions should compare old basic and new acoustic guitars at matched levels
with identical notes and velocities, including low/mid/high notes, chords,
short/long gates, bends, and vibrato. Record both the matching gains and what
the listener heard. Until the user listens, perceptual acceptance remains
pending. Finite audio, pitch estimates, or successful WAV generation do not
establish realism or listening approval.
