# Implementation decisions and specification questions

The MaaC specification remains the normative language document. These notes
identify places where implementation needs an explicit interpretation or a
future specification clarification. They are not a claim of complete language
conformance.

## Local libraries and synthesis versions

The [instrument extension](instruments.md) specifies the reusable sound-library
boundary separately from the legacy core palette. Its `synth.* /1` identities
pin phase reset, table interpolation, harmonic-bank switching, ADSR sampling,
and strict parameter ranges. In particular, `synth.pan/1` rejects out-of-range
pan while `core.pan/1` retains its existing clamp policy.

Source bundles contain bytes and project-relative identities; filesystem
containment is enforced by the loader. The compiler consumes the in-memory
bundle, and version 2 plans embed the programs and raw wavetables needed by the
renderer. Hashes identify source dependencies rather than authorize execution.
No executable plugin or network resolution is part of this extension.

The filesystem loader uses `cap-std` directory capabilities for contained opens
on the supported platforms. This replaces a canonicalize/check/path-reopen
sequence that could follow a changed path component outside the selected root.
Canonical-relative mapping preserves existing contained symlinks; type checks
and bounded reads use the already-open handle. Unix opens add `O_NONBLOCK`
before rejecting nonregular files so a FIFO cannot stall loading. The added
dependency keeps platform-specific containment logic in a maintained library.

Public controls inherit their target's units, range, and automation rate.
Note-on and note-off controls support global automation through event-time
capture. Graph modulation is sample-rate only. Voice instances share immutable
compiled programs and table banks while retaining their own mutable DSP state.
Shared effects continue through the declared tail. These rules preserve the
foundation's event schedules and explicit rendering endpoint.

## Plucked-string processor contract

The [approved plucked-string contract](plucked-string.md) specifies private recirculating
delay state in one mono voice-only `synth.pluck/1` processor while retaining DAG
validation for public audio and modulation edges. It uses deterministic seeded
excitation, live phase-compensated convex delay interpolation, and bounded
frequency-dependent loss. Separate heap rings, declared-capacity memory checks,
and initialization/weighted-sample work prevent hidden resource expansion.
Combined-loop damping need not be monotonic, and DSP failures promise only
node-local validation before state updates, not whole-frame rollback.

The separate `std/acoustic/1.0.0` library preserves frozen basic sounds and
existing default catalog behavior. The contract excludes a new body-resonator
primitive and a realism claim without user listening. Contract acceptance and
implementation evidence remain separate from perceptual acceptance.

## Project end and physical offsets

Pattern/use/placement `cut` boundaries truncate musical gates before physical
offsets, as section 9 states. At project end, the foundation applies the section 6
effective-time formula and then truncates the effective release:

`off_seconds = min(T(final_score_off) + release_offset, T(score.end))`.

The plan retains `final_score_off` before this project-end truncation. For a
120-bpm score ending at `1q`, a note with musical end `2q` and release offset
`-250ms` therefore releases at 0.5 seconds. Clamping its score coordinate first
would incorrectly release it at 0.25 seconds. The specification should state this
distinction between repetition cuts and project-end effective truncation directly.

## Pan range policy

Section 18.3 declares both a pan range of `[-1,1]` and a clamp policy. The
foundation preserves finite raw numeric pan values and clamps the evaluated
parameter to that range. It uses the declared error policies for sine parameters
and one-pole cutoff. A future descriptor schema should distinguish accepted raw
values from the post-policy range explicitly.

## Derived plan and numeric fidelity

The existing syntax-tree JSON is not the performance-plan format. Plan versioning
is independent of source-language versioning. Source/timing quantities remain
exact rationals; resolved pitch is binary64 because tuning and cents operations
generally produce irrational frequencies. This separates exact scheduling from
the DSP numeric representation without claiming exact acoustic arithmetic.

## Single-input ports

Only `core.sum/1` explicitly permits empty audio input. The foundation requires a
connection to `core.pan/1` and `core.onepole/1` single audio inputs. The specification
should spell out `zero_default` for each reference processor descriptor rather
than leaving it implicit in prose.
