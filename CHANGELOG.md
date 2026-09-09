# Changelog

This file records user-visible changes. The project is experimental and has no
promise of compatibility beyond the documented interfaces.

## [Unreleased] — experimental v0.1.0 source preparation

This entry describes the current source tree prepared for a GitHub source-only
release. It may be moved to a dated `0.1.0` release entry when a release is
actually published; no release tag or publication date is asserted here.

### Added

- Per-note gain expression on `core.sine/1`, with nonnegative amplitude curves,
  step/linear/exponential interpolation, and simultaneous pitch expression.
  Zero gain preserves voice state. Optional `gain_expression` extends plan
  versions 1 and 2; absent gain retains previous JSON and audio behavior.
  See the [gain guide](docs/gain-expression.md).
- Per-note pitch expression on `core.sine/1`, with cents-based step/linear
  curves on normalized, seconds, or score clocks and independent release
  holding. Optional note payloads extend plan versions 1 and 2 while preserving
  expression-free JSON. See the [authoring guide](docs/pitch-expression.md).
- Experimental native `fx.eq/1`, `fx.compressor/1`, and `fx.reverb/1`, with
  strict source and retained-plan validation, sample automation, reset replay,
  external sidechains, and bounded complete-graph master/stem capture.
- Named `maac deliver` source/retained-plan workflows with 44.1/48/96 kHz
  resampling, Float32/PCM24/PCM16 WAV, explicit deterministic TPDF dither,
  final-artifact analysis, manifests, and retained audio after failed limits.
  Native tags and optional `Plan.production` extend plan versions 1 and 2;
  existing valid JSON remains unchanged. New Rust struct fields require
  exhaustive literal updates. The [specification](docs/production.md), pinned
  schema, examples, and bounded fixture checker remain distinct from renderer
  acceptance. The [metering audit](docs/production-metering-evidence.md) records
  applicable current 4× fixtures and historical 16× failures. Analyzer identity
  `maac.analysis.bs1770-5/2` selects the approved single-stage Annex 2 profile
  `maac.truepeak.bs1770-5.annex2-4x/1`; rendered audio is unchanged by this
  analysis revision. No full ITU/EBU or professional sound-quality claim is made.
- A Rust library and `maac` CLI for checking source, compiling a standalone
  versioned performance plan, rendering an imported plan, and building source
  directly to WAV.
- Exact rational source timing, explicit routing, stable event identities,
  finite pattern expansion, global automation, and the four documented
  foundation processors.
- Float32 WAV export by default and overload-rejecting PCM16 export, with
  atomic output publication and overwrite protection.
- Independent validation of imported plans, bounded inputs and collections,
  deterministic repeat-render checks within one executable and environment,
  and the asset-free `example.maac` and `evening-window.maac` examples.
- A Python syntax and selected-semantics smoke checker plus an offline installed
  CLI acceptance runner.
- Reusable local sound libraries with pinned imports, typed controls, presets,
  and independent polyphonic voice/shared graphs. The synthesis palette adds
  basic oscillators, ADSR, LFO, feed-forward FM, explicit morphing wavetables,
  gain, one-pole filtering, mixing, and panning.
- In-memory bundle APIs, library-export checking, a read-only `maac hash`
  helper, and standalone version 2 plans with embedded graph/data/provenance.
  Version 1 rendering remains supported.
- A shared FM bell, wavetable pad, and bass library, reusable presets, and a
  four-instance composition, with analytic and spectral regression fixtures.

### Changed

- Project `bar(...)` crop coordinates now resolve against the declared meter
  before compilation; a 3/4 bar interval matches its exact q spelling and
  normalized execution identity.
- Delivery filenames now preserve distinct uppercase/lowercase IDs on
  case-insensitive filesystems and bound long components with stable SHA-256
  suffixes. Duplicate destination names fail before rendering, including when
  overwrite is authorized; ordinary lowercase names retain their spelling.
- Project branding, the Rust crate, the installed command, source-file extension,
  and document header use MaaC naming (`maac` / `.maac` / `maac 1;`).
- The draft `core.noise/1` hash prefix is now `maac-noise-1`, changing its
  deterministic reference values. The renderer foundation does not implement
  this processor.

### Limitations

- The Rust implementation is a foundation subset; it does not claim full
  Document, Performance, Core Audio, or Locked Render conformance.
- Tempo ramps, per-note pressure/timbre expression, pitch/gain expression on
  graph instruments, hits/messages, top-level core modulation,
  other processors, recorded-sample instruments, arranged audio, external plug-ins,
  transactional editing, MaaC Locked Render dependency-lock manifests
  (distinct from `Cargo.lock`), MIDI transport, GUI, and real-time playback
  remain deferred or outside the current interfaces. Recognized deferred
  features fail explicitly with `E_CAPABILITY`.
- The normative specification remains a design draft and needs
  implementation-driven review before stabilization.
- Automated finite, non-silent sample measurements and repeatability checks do
  not establish cross-platform bit identity or human listening quality. The
  listening review is pending.
- This is a source-only preparation; generated binaries and rendered audio are
  not included. The starter library includes its small authored wavetable WAV.
