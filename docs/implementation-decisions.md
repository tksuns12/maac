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
capture. Graph modulation supports sample-rate parameters and
[voice ADSR/phase and shared-LFO reset parameters](internal-event-modulation.md). Internal release
capture uses a non-advancing pre-release snapshot; normal audio uses post-event
state and advances each processor once. Internal event sums are checked only
when captured, unlike top-level modulation's per-frame validation. Voice instances share immutable
compiled programs and table banks while retaining their own mutable DSP state.
Top-level reset controls capture required control dependencies at frame zero
before instrument construction; non-reset controls retain authored values for
the internal reset preview. Shared effects continue through the declared tail. These rules preserve the
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
would incorrectly release it at 0.25 seconds. [MaaC-1 §6](../MaaC-1-Specification.md#6-time-offsets-and-scheduling)
states this distinction directly; the existing foundation behavior is unchanged.

## Pan range policy

[MaaC-1 §18.3](../MaaC-1-Specification.md#183-corepan1) defines finite raw pan
inputs, retention of those values and automation endpoints, and one clamp after
base, replacement, and modulation values are combined. `synth.pan/1` retains
its separate strict extension contract. Generic descriptor representation of
raw versus effective ranges remains L4 work; the existing foundation behavior
is unchanged.

## Derived plan and numeric fidelity

The existing syntax-tree JSON is not the performance-plan format. Plan versioning
is independent of source-language versioning. Source/timing quantities remain
exact rationals; resolved pitch is binary64 because tuning and cents operations
generally produce irrational frequencies. This separates exact scheduling from
the DSP numeric representation without claiming exact acoustic arithmetic.

Nonconstant tempo ramps generally produce irrational physical times. Version 3
therefore retains exact score positions, physical offsets, and the tempo map,
without the legacy rational `on_seconds`/`off_seconds` fields. Bounded outward
logarithm enclosures certify discrete frame ceilings; the DSP inverse clock uses
floating point only after those transitions are prepared. Legacy step-only
plans and public Rust types retain their contracts through additive versioned
APIs. [Tempo ramps](tempo-ramps.md) records the numerical and compatibility rules.

## Single-input ports

[MaaC-1 §15](../MaaC-1-Specification.md#15-nodes-ports-and-connections) and its
core input table define single versus summing cardinality, explicit
`zero_default` behavior, and the `E_PORT_TYPE` result for missing or incompatible
inputs. Event empty-stream behavior is separate from audio/control zero defaults.
The §17 descriptor contract carries the same fields for external processors.
The bounded L4 `maac::external` boundary now defines a strict descriptor wire,
verifies complete processor-owned locked dependency closure bytes, and requires
explicit host ABI, adapter, permission, and determinism authorization. It does
not define or execute a native calling ABI; executable hosting remains separate
work.

## Tuning reference contract

[MaaC-1 §7](../MaaC-1-Specification.md#7-pitch-and-tuning) defines all four
tuning fields as required, with no defaults. `reference_index` is any
dimensionless mathematical integer; a declared signed-64 host limit is a
resource limit rather than an array or MIDI bound. `reference_frequency` is
finite and strictly positive, with Hz and equivalent kHz units accepted.
Missing, nonintegral, and nonpositive values use `E_RANGE`; wrong units use
`E_UNIT`; an exact value that cannot become a finite host frequency uses
`E_NONFINITE`; and a declared integer representation limit uses
`E_RESOURCE_LIMIT`. The tuning slice in
[`tests/l3_tuning.rs`](../tests/l3_tuning.rs) and
[`conformance/l3/tuning/`](../conformance/l3/tuning/expected.json) records the
public compile, retained pitch, render-replay, and diagnostic-path evidence.
