# Acoustic guitar delivery evidence

The frozen `std/acoustic/1.0.0` collection and `synth.pluck/1` passed the
numerical and installed-authoring checks described here on the local macOS
environment. **User listening acceptance remains pending.** These results do
not claim subjective acoustic realism or cross-platform bit identity.

## Frozen inputs and installation

Acoustic source SHA-256 is
`31d3b0a95ccc6f3b365d20337616f7259d72c19cbc2debddb63304a8c19a689a`.
The unchanged basic source hash is
`cc7d6e307ed28810f75cdd5c8aa1ed24b08d7db94f78d88cc293d0ceb7ba397b`.

A fresh repository-local installation used:

```sh
cargo install --target-dir target/acoustic-install-build --jobs 2 --path . \
  --root target/acoustic-install --locked --offline
```

The installed `maac` binary hash is
`0a8fd82a911106aa451b4995aece9779952b09d9ac71d550b17bbb28b442531c`.
Its separate build directory preserved the release binary used by concurrent
audio work. The pre-acoustic installation remained unchanged at hash
`251698a666b34ab3fe86e214e2413f89cf36e25e8b3b62129b11bf50c26b3ba1`.
All compiled source/stdlib/Cargo inputs stayed unchanged during authoring checks.

The existing CLI and reusable-instrument acceptance scripts were copied into
the disposable evidence directory. Only installation/artifact paths, separate
build-directory/job arguments, and stale agent attribution were adjusted;
their assertions were preserved. Original scripts and the pre-acoustic
installation were not edited. Their nested run performed the fresh installation.

## Observed checks

| Boundary | Observed result | Evidence |
| --- | --- | --- |
| Integrated repository gates | 293 tests passed, zero failed/ignored; formatting, all-target Clippy and release build passed on unchanged inputs | [Gate summary](../target/acoustic-acceptance/gates/summary.json) |
| Isolated string kernel | 12 tests passed | [Kernel metrics](../target/acoustic-acceptance/gates/kernel-metrics.json) |
| Source, graph, runtime and plan integration | 11 integration tests plus two integration unit tests passed | [Integration report](../target/acoustic-guitar-design/pluck-integration-evidence.json) |
| Acoustic instruments | Five tests passed, all 49 E2–E6 semitones per export, endpoint bends/vibrato, long gates and chords | [Instrument report](../target/acoustic-guitar-design/instrument-validation.json) |
| Existing installed CLI acceptance | 38 checks, 24 command records passed | [CLI report](../target/acoustic-acceptance/installed/legacy/results.json) |
| Existing reusable-instrument and disposable spec acceptance | 31 checks, 12 command records passed | [Reusable/spec report](../target/acoustic-acceptance/installed/reusable/results.json) |
| Installed acoustic authoring and compatibility | 319 checks, 60 command records passed | [Authoring report](../target/acoustic-acceptance/installed/authoring-results.json) |

Checks include repeated binary fingerprints and other boundary assertions;
their count is not the number of distinct compositions or audible tests.
Kernel/integration/instrument test counts are subsets of the integrated suite,
not extra tests to add to 293. The [installed summary](../target/acoustic-acceptance/installed/report.json)
links the distinct installed acceptance scopes.
The disposable specification checker used Python 3.12.14, Lark 1.2.2 and
jsonschema 4.25.1. Its three generated reference files matched tracked bytes,
without overwriting those tracked files.

Kernel signal-pitch estimates at E2, E4 and E6 were 0.00 cents on the tested
0.25-cent search grid. The independent estimator used a 16,384-sample Hann
Fourier scan after 100 ms. This is a grid-qualified estimate, not exact acoustic
frequency. All E2–E6 semitones with down/unchanged/up two-semitone bends, plus
20/4000 Hz endpoints at damping 0/0.5/1, passed the separate phase-equation
residual threshold of `1e-12` radians. No measured maximum residual was logged.

The upper/lower partial-energy ratio fell from 1.375044 to 1.045154 in the
kernel fixture. A 96,000-frame changing-parameter stress test retained finite
internal state within `1+1e-12`. These observations support the specific decay
and stability contracts; they do not make combined-loop damping monotonic.

## Installed authoring outside the checkout

The fresh installed binary ran in an otherwise empty temporary directory
outside the checkout. Discovery covered both exact libraries, selected
catalog/detail, six acoustic controls, unknown library/name errors, and
conflicting selector arguments. No source library or audio asset was copied
into that directory.

All nine sources below passed check, PCM16 build, repeat build, compilation,
and retained-plan rendering after their temporary composition was removed.
Each output was finite, nonzero, stereo, and 48 kHz; original, repeated and
retained-plan WAV bytes matched exactly on this host.

| Source | Duration | PCM16 normalized peak |
| --- | --- | --- |
| Catalog usage: nylon | 5 s | 0.12876 |
| Catalog usage: steel | 5 s | 0.12882 |
| Catalog usage: muted | 5 s | 0.09870 |
| Nylon audition | 7.5 s | 0.13031 |
| Steel audition | 7.5 s | 0.12381 |
| Muted audition | 7.5 s | 0.09897 |
| Fingerpicked phrase | 5.7 s | 0.15250 |
| Guide composition | 3 s | 0.09656 |
| Guide with smooth bend and vibrato 0.3 | 3 s | 0.09864 |

Normalized PCM16 peaks divide integer magnitude by 32767. They differ slightly
from pre-export float measurements. The complete inputs, compiled plans, WAVs,
command logs and fingerprints accompany the authoring report.

Default basic catalog and nylon-detail JSON matched the pre-acoustic installed
binary byte-for-byte. The old basic nylon audition also rendered identical
PCM16 WAV bytes with both executables: 182,400 frames / 3.8 seconds, SHA-256
`48d7a437377c546a27015235319b2a3f66f1e2b007b44f8597fa10ab78bbc0e0`.
This package did not rerender all 25 basic auditions; existing CLI/reusable
compatibility checks and this explicit old/new guitar comparison ran instead.

## Limits of acceptance

The tests do not establish a complete guitar-body model, pick position,
velocity-dependent excitation timbre, random variation between repeated
same-seed notes, or quality under arbitrary discontinuous automation.
Signal-derived pitch was measured at three representative notes; full semitone
coverage used rendered range tests and the independent phase equation.

The [author guide](acoustic-guitars.md) describes supported controls and the
[implementation contract](plucked-string.md) fixes processor behavior. A
level-matched user listening comparison remains necessary before claiming
improved acoustic realism. Generated audio and numerical results are not a
substitute for that assessment.
