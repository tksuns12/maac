# MaaC playable foundation implementation plan

This plan records the technical decisions and acceptance boundary for the MaaC
playable foundation. The [ScoreIR-1 specification](../ScoreIR-1-Specification.md)
remains the normative language document. The foundation is an intentionally
bounded implementation subset, not a full conformance profile.

## Product boundary

The foundation provides a Rust library and MaaC's `maac` CLI that parse and
check ScoreIR source, compile it to an independently loadable performance plan,
and render that plan to WAV. Compilation and rendering are offline operations.
The CLI owns filesystem access and atomic destination publication; parser,
semantic, compiler, plan, DSP, and export library boundaries stay explicit.

## Technical decisions

### Source and resolution

- Source values and musical timing use exact rational arithmetic. Frame counts
  use mathematically correct ceilings, with precision failures reported rather
  than guessed.
- Patterns are finite. Nested uses, repetition, stretching, cuts, spills,
  transposition, overrides, and inserts resolve to stable occurrence addresses
  before scheduling.
- Tracks, placements, note targets, audio connections, processor parameters,
  tempo, meter, and automation are explicit. There is no implicit mixer,
  channel conversion, or silent approximation of an unsupported feature.
- Recognized deferred features fail with `E_CAPABILITY`. Resource, version,
  timing, and validation failures remain explicit diagnostics.

### Plan interchange

- The derived performance plan has its own version and is independent of the
  ScoreIR source-language version.
- Plan import validates the complete artifact again, including versions,
  references, identities, numeric ranges, resource bounds, graph structure,
  and timing/frame consistency. An imported plan is treated as untrusted input.
- Source mappings and resolved exact timing remain in the plan where needed for
  inspection, while authored source remains the source document's concern.

### Audio and export

- The reference foundation processors are `core.sine/1`, `core.onepole/1`,
  `core.pan/1`, and `core.sum/1`.
- DSP operates in binary64 and resets processor state for each render. The
  renderer streams frames to a fallible callback so callers need not accumulate
  a complete render in memory.
- WAV export defaults to 48 kHz float32. PCM16 first rejects nonfinite or
  out-of-range samples and then applies the documented quantization. Export
  does not normalize, limit, or dither.
- Output files are published atomically only after a successful render. Existing
  destinations require `--force`; failed operations preserve the destination.

### Bounds and determinism

The [capability matrix](capabilities.md) is the source of truth for input,
expansion, graph, rational, voice, and render-work limits. Exceeding a limit
must produce an explicit diagnostic without dropping notes, reducing precision,
or changing graph meaning. Determinism is asserted within the tested executable
and environment; cross-platform bitwise identity is not promised.

## Acceptance boundary

The implementation is accepted when the following behaviors are covered by
automated tests and release checks:

- Both authored compositions parse, resolve, and render: `example.maac`
  contains 48 notes and 816,000 stereo frames; `evening-window.maac`
  contains 182 notes and 1,968,000 stereo frames at 48 kHz, including explicit
  one-second tails.
- Exact timing, pitch, pattern expansion, overrides, routing, automation,
  processor algorithms, voice allocation, and plan-import validation obey the
  decisions above.
- The CLI supports check, compile, render, build, structured diagnostics,
  float32/PCM16 selection, overwrite protection, and failure-safe destinations.
- Compilation and rendering perform no network access, and repeated renders
  produce identical PCM in the tested executable and environment.
- Formatting, Clippy, the complete Rust test suite, release build, Python
  smoke check, and installed-CLI acceptance checks pass on the documented
  macOS stable-Rust baseline.

The detailed cases are in the [acceptance cases](acceptance.md). The
[verification report](verification.md) records commands, observed outcomes,
environment provenance, artifact measurements, and the separate listening
status. Automated measurements do not count as a listening review.

## Implementation sequence

The implementation is organized around independently testable boundaries:

1. Parse source while retaining authored text, typed values, spans, and exact
   numeric forms.
2. Validate source structure, units, references, capabilities, and resource
   limits, then resolve musical clocks and finite pattern occurrences.
3. Serialize and independently validate the versioned performance plan.
4. Lower resolved events into the documented processors, render reset-state
   audio, and export WAV without changing signal gain.
5. Expose the safe CLI operations and verify examples, failures, repeatability,
   and offline installation at the release boundary.

## Deferred work

The foundation does not silently reserve behavior for future versions. The
unsupported and deferred feature list, resource limits, and fidelity caveats
remain in [capabilities.md](capabilities.md). Specification questions that
need an explicit future interpretation are tracked in
[implementation-decisions.md](implementation-decisions.md).
