# Changelog

This file records user-visible changes. The project is experimental and has no
promise of compatibility beyond the documented interfaces.

## [Unreleased] — experimental v0.1.0 source preparation

This entry describes the current source tree prepared for a GitHub source-only
release. It may be moved to a dated `0.1.0` release entry when a release is
actually published; no release tag or publication date is asserted here.

### Added

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

### Changed

- Project branding, the Rust crate, the installed command, source-file extension,
  and document header use MaaC naming (`maac` / `.maac` / `maac 1;`). The
  ScoreIR language name remains unchanged.

### Limitations

- The Rust implementation is a foundation subset; it does not claim full
  Document, Performance, Core Audio, or Locked Render conformance.
- Tempo ramps, per-note expression, hits/messages, modulation, additional
  processors, sample assets, arranged audio, external plug-ins, extensions,
  transactional editing, ScoreIR Locked Render dependency-lock manifests
  (distinct from `Cargo.lock`), MIDI transport, GUI, and real-time playback
  remain deferred or outside the current interfaces. Recognized deferred
  features fail explicitly with `E_CAPABILITY`.
- The normative specification remains a design draft and needs
  implementation-driven review before stabilization.
- Automated finite, non-silent sample measurements and repeatability checks do
  not establish cross-platform bit identity or human listening quality. The
  listening review is pending.
- This is a source-only preparation; generated binaries, WAV files, and other
  release artifacts are not included.
