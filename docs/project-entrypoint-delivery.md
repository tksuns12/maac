# Project entrypoint delivery evidence

The conventional `main.maac` entry, explicit finite `song` execution profile,
selected-limit Rust APIs, and existing `core.gain/1` processor are implemented.
The [normative contract](project-entrypoint.md) defines their behavior; this
report records the local validation observed on 2026-09-08. A historical local
composition fixture built and rendered from a source-free retained plan with
identical WAV bytes. Listening was not performed in this validation pass, and
these results make no cross-platform or perceptual claim.

## Integrated and installed checks

The final frozen implementation passed `cargo fmt --check`, all-target Clippy
with warnings denied, all 323 tests across 41 result groups (zero failed or
ignored), a release build, and a fresh repository-local offline installation.
The 69 build-input fingerprints were unchanged across those gates;
the previous installations were preserved. Exact commands, logs, hashes, and
results are in [the gate report](../target/project-entrypoint-validation/summary.json).

The installed executable passed 206 outer acceptance checks, plus the existing
38-check legacy and 31-check reusable-instrument harnesses. Checks ran from a
disposable directory outside the checkout. They covered entry selection and
containment, explicit profile behavior, source-free plans, generated
specification files, and compatibility. Disposable specification outputs were
byte-identical to the checked-in artifacts; established core WAV hashes and
catalog stdout pins were unchanged. See [installed acceptance](../target/project-entrypoint-validation/installed-acceptance/results.json).

The complete MaaC example in the [entrypoint guide](project-entrypoint.md) was
checked, compiled, and built using omitted input, then its source was removed.
Retained renders matched their built WAVs byte-for-byte in both float32 and
PCM16. Each contains three notes and 144,000 stereo frames at 48 kHz (3 seconds),
with finite, nonzero output. Float32 peak was 0.06760285049676895; PCM16 peak was
2215/32768. The executable source fence hash is
`05be85b585426b41240f7f5dac4ffb8c1b7b3b06e3f7972d2b547a6d79c07f80`.

## Historical local fixture

A local composition fixture with explicit final master gain was used for the
following tool-validation checks; the fixture is not distributed with the
repository. Its plan audit verified 2,399 notes, 17 tempo points, 24 instrument
lanes, and 57 controller bindings
containing 3,235 points, including exact timing, pitch, velocity, routing, and
control data. The serialized plan is 1,527,620 bytes and schedules 9,620,109
frames. Its estimated instrument work is 4,165,619,697 units: above the unchanged
500-million default, within the explicit 10-billion song allowance. Delay
storage uses 93,678 cells (749,424 payload bytes), within the unchanged pluck
memory bound. See [the native plan audit](../target/soldier-of-fortune-main/native-plan-audit.json).

A byte-identical copy of the freshly installed CLI ran outside the checkout
with only `main.maac` as source. Omitted-input check, compile, and build passed;
the full native PCM16 build took 142.277 seconds. After the source was removed,
a default-profile retained render rejected the plan with `E_RESOURCE_LIMIT`
and preserved an existing output even with `--force`. Explicit song-profile
retained rendering passed in 142.846 seconds and produced a byte-identical WAV.

The output has 9,620,109 stereo PCM16 frames at 48 kHz (200.4189375 seconds),
finite samples, zero clipped samples, normalized peak 0.8499710074159978, and
RMS 0.10853027373425388. Compared with the historical acoustic rendition,
1,312 of 19,240,218 samples differ, by at most one PCM16 count; RMS difference
is 0.008257753662750915 counts. This satisfies the declared rounding bounds
of at most two counts and 0.1 RMS counts. It is numerical comparison evidence,
not a listening assessment. See [native verification](../target/soldier-of-fortune-main/native-verification.json),
[standalone plan audit](../target/soldier-of-fortune-main/standalone-plan-audit.json),
and [exact commands](../target/soldier-of-fortune-main/standalone-commands.json).

Final identities were rechecked unchanged:

| Artifact | SHA-256 |
| --- | --- |
| Master source | `9834e5fb77540d6543aab956e2accb0c6d224433bdb71708298582ec912bd2d0` |
| Installed executable | `6d15599adbca2fe99b206fb5461577f22b7211549417debe206f97edfc889e80` |
| Retained plan | `3cc8bffefc75a6aa75ac10ed032561452960a5eb8bb6e087032f00c2fa4b2a5b` |
| Native and retained WAV | `301862d30d2e43c45e63297c0f4d29b1e262e6ece19ce4c24a085a26a5252001` |

## Compatibility boundaries

Explicit input filenames and default Rust wrappers retain their behavior.
Render/hash still require an explicit file; only source commands discover
`main.maac`. Sources and plans cannot select their own allowance. A large
retained plan requires caller-selected song limits again when loaded or rendered.
No language grammar, import semantics, plan wire version, or other resource
ceiling changed.

Both frozen built-in sources retain their identities:

| Library | Source SHA-256 |
| --- | --- |
| `std/basic/1.0.0` | `cc7d6e307ed28810f75cdd5c8aa1ed24b08d7db94f78d88cc293d0ceb7ba397b` |
| `std/acoustic/1.0.0` | `31d3b0a95ccc6f3b365d20337616f7259d72c19cbc2debddb63304a8c19a689a` |

Detailed reports and WAVs linked under `target/` are local generated evidence,
not checked-in release artifacts. This document preserves their measured
results without requiring those generated files to be distributed.
