# Changelog

This file records user-visible changes. The project is experimental and has no
promise of compatibility beyond the documented interfaces.

## [Unreleased] — experimental v0.1.0 source preparation

This entry describes the current source tree prepared for a GitHub source-only
release. It may be moved to a dated `0.1.0` release entry when a release is
actually published; no release tag or publication date is asserted here.

### Added

- Pitched sample instruments: library `sample` declarations and the voice-only
  `synth.sample/1` processor play recordings at their recorded rate from a
  12-TET root key, with forward sustain loops, overlapping key zones, velocity
  layers, and linear or equal-power key and velocity crossfades. Samples may be
  mono or stereo. They come from an embedded WAV, a core PCM file carried as a
  plan audio asset (inline or with `--disk-media`), or a composition audio
  asset shared with clips and kits. See [samples](docs/instruments.md#samples).
- Audio clips inside patterns: `audio` leaves in patterns and placement
  inserts repeat, nest and stretch with their pattern. A placement exposes an
  explicit `out` port, and overrides may delete an occurrence or change its
  timing, gain or fades. Each placement is one plan node however many
  occurrences it holds. See the [pattern audio contract](docs/pattern-audio.md).
- Arranged audio: top-level `audio` clips in `rate` mode (speed, reverse,
  musical or physical placement, gain, linear or equal-power fades) and
  tempo-aware `warp_rate`, plus `warp_preserve` through the core
  `core.stretch.ola/1` reference stretch. Plan versions 5 and 6 carry them. See
  the [audio clip](docs/audio-clips.md) and [warp](docs/warp-rate.md) contracts.
- Core control modulation: `modulate`, `core.lfo/1` and `core.constant/1`,
  with additive contributions, event-rate and reset-rate capture, and
  instrument graph modulation including ADSR, voice phase and shared-LFO reset
  capture. Plan version 7 carries controls. See the
  [modulation contract](docs/core-modulation.md).
- Core processors `core.fader/1` (decibel level), `core.matrix/1`,
  `core.delay/1` (explicit delays permit causal feedback) and the
  counter-derived reference `core.noise/1`. See [fader](docs/core-fader.md),
  [matrix](docs/core-matrix.md), [delay](docs/core-delay.md) and
  [noise](docs/core-noise.md).
- Message events resolve into retained transport events with exact protocol
  bytes, certified frames and §6.1 ordering. Hosts dispatch them through
  `PlanArtifact::performance_dispatches`; the built-in renderer has no
  raw-message adapter.
- Protocol 2 transactional editing through `maac::editing` and `maac patch`:
  source-preserving atomic edits with inverses, bounded edit impact, edits of
  local and built-in imports and library sources with atomic repinning, and
  diagnostics carrying original-source byte ranges. See the
  [editing kernel](docs/editing-kernel.md) and
  [editing diagnostics](docs/editing-diagnostics.md).
- `maac materialize-instance` copies a placement into independently editable
  patterns with a complete source-address mapping. See the
  [materialization contract](docs/materialize-instance.md).
- `maac query-events` and `PlanArtifact` score-window queries return complete
  events intersecting an exact half-open window. See
  [event queries](docs/windowed-event-query.md).
- Reusable musical declarations: pinned pattern, curve and tuning exports
  resolve in their declaring library, and `maac module` exports, validates and
  unpacks `maac.module-source/1` source modules. See
  [reusable declarations](docs/instruments.md#reusable-musical-declarations)
  and the [module artifact](docs/musical-module-artifact.md).
- Generic interchange: canonical generic Locked Render lock generation and
  verification, rendering a verified lock for built-in plans, strict external
  processor descriptors, and the `maac.native-c-abi/1` host for one
  output-only external generator. See [generic interchange](docs/generic-interchange.md)
  and the [native ABI](docs/native-external-abi-v1.md).
- MIDI 1.0 SMF export with explicit loss reports. See
  [loss reporting](docs/interchange-loss-report.md).
- `maac render` accepts reset-origin frame bounds for exact WAV excerpts that
  still execute the complete plan. See [range rendering](docs/render-range.md).
- Opt-in disk media: `check`, `build` and `patch` accept `--disk-media` to use
  hash-pinned native PCM up to 1 GiB through private snapshots. See the
  [disk-media contract](docs/disk-media.md).
- `maac import-wav` and `maac verify-import` import whole mono or stereo WAVs or
  frame crops (PCM16/24/32 and float32) into a relocatable project with
  provenance, optionally retaining the original file. See
  [WAV import](docs/media-import.md).
- Native composition archives: `maac archive create/patch/verify/unpack`
  capture exact source and media closures. Versions 2–12 add checkpoints,
  retained WAV imports, verified output and native-effect freezes with explicit
  reuse, journaled entry, library and shared-source edits, and retained native
  processor context. See the [archive contract](docs/editable-archive.md).
- Takes and comping through `maac.takes/1` and grouped microphone lanes through
  `maac.takes/2`. See [takes](docs/takes-and-comping.md) and
  [grouped takes](docs/grouped-takes.md).
- macOS process tools: `maac play` renders and auditions through the default
  output; `maac inputs` lists devices; `maac record` captures bounded 48 kHz
  mono takes from the default or a UID-selected input, with opt-in duplex
  monitoring. See [playback](docs/playback.md), [recording](docs/recording.md)
  and [input monitoring](docs/input-monitoring.md).
- Language conformance evidence: the specification adds edit protocol 2 with
  authored revision identity, timing coordinates, tuning and processor input
  contracts, generic interchange, and bounded L1–L5 quantitative conformance
  corpora. The [evidence map](docs/document-performance-evidence.md) records
  public tests, including a grammar-driven lexical corpus checked against
  `grammar.lark`, spec-derived structure and value-type inventories, the L1
  metadata-role fragment run through the execution projection, a §23
  diagnostic catalog and region vectors.
- A sampler, warp and stereo [feature showcase](examples/showcase/) with
  recorded informal [listening approvals](docs/showcase-listening.md).

- Native hits and `core.kit/1` one-shot mono/stereo sample playback using existing
  syntax. Pinned raw float32 assets retain their original rate and exact bytes;
  version 4 plans embed them for standalone rendering and named deliveries.
  Additive opaque artifact APIs preserve existing public plan and processor
  types. See the [kit contract](docs/core-kit.md).

- Linear-in-score tempo ramps using existing MaaC/1 syntax, with certified
  scheduling, inverse-clock automation, standalone version 3 plans, and named
  deliveries. Additive versioned Rust APIs preserve the legacy plan types and
  step-only entry points. See the [tempo contract](docs/tempo-ramps.md).

- Per-note pressure for custom voice graphs declaring `synth.pressure/1`.
  Independent exact 0…1 curves use authored graph mappings and hold through
  release. Pitch, gain, timbre, and pressure can coexist on opted-in notes.
  Optional strict `pressure_expression` payloads preserve absent-field JSON
  and plan version 2; frozen libraries remain unchanged. See the
  [pressure guide](docs/pressure-expression.md) and standalone example.

- Per-note timbre for custom graphs declaring voice-only `synth.timbre/1`.
  Exact 0…1 curves map through existing signed-depth modulation, coexist with
  pitch and gain, and hold through release. Optional `timbre_expression` note
  payloads preserve absent-field JSON; graph plans remain version 2. Frozen
  basic/acoustic libraries do not opt in. See the [timbre guide](docs/timbre-expression.md).

- Per-note pitch on reusable mono/stereo instruments, including basic, acoustic,
  and custom graphs. Independent cents curves preserve oscillator and string
  state, coexist with gain, and reuse existing source and plan fields. See the
  [instrument pitch guide](docs/instrument-pitch.md) for live frequency limits.

- Per-note gain on reusable mono/stereo instruments, including basic, acoustic,
  and custom graphs. Gain follows the complete voice contribution and precedes
  shared effects; zero gain preserves DSP and voice lifecycle. Existing curve
  syntax and plan fields are reused, with a bounded additional execution-work
  charge. See the [instrument gain guide](docs/instrument-gain.md).

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

- Source validation is stricter where it was more lenient than the
  specification. A `use` requires `at`; objects nested in a `project` or a core
  processor `node` are refused instead of ignored; lowercase pitch spellings
  such as `c4`, a nonintegral `key()` and a nonpositive `ratio()` are refused;
  and a warp anchor with the wrong unit is `E_UNIT` rather than `E_RANGE`.
  Sources that relied on the old behavior now fail validation.
- Diagnostics report §23 locations: object paths contain only authored IDs, and
  field paths name the field and list position, such as `["points", "1"]`,
  `["set", "at"]` or `["params", "level"]`. Codes are unchanged except for the
  warp anchors above. Tools that matched the earlier labels see new paths.
- In `grammar.lark`, the `maac` keyword and pitch tokens end at an identifier
  boundary, matching the Rust parser: `maac1;` is not a header and `C4x` is one
  symbol.
- Global `bar(b,u)` positions are lowered through the project meter during
  source validation, so reversed or out-of-score bar spans fail before
  compilation.
- A placement on a track without an event target is accepted when it expands
  only audio leaves.
- A caller-tightened channel limit is reported as `E_RESOURCE_LIMIT`;
  unsupported channel capabilities remain `E_CAPABILITY`.
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
  deterministic reference values.

### Fixed

- Unknown extensions are reported at their own `namespace` field everywhere:
  `E_CAPABILITY` for an unsupported namespace and `E_REFERENCE` for any
  namespace that `project.requires` does not list. Bundle checks previously
  reported them at a production path, and production and takes reported an
  unlisted namespace with `E_CAPABILITY`. `execution_identity` refuses
  documents with an unsupported extension namespace instead of hashing them,
  and `maac::extensions::SUPPORTED` lists the supported identifiers.

- Omitted required `config` on `core.sum/1` and `core.onepole/1` is refused
  during source validation, not only at compilation.
- Same-sample graph cycles are refused before an edit commits or `maac patch`
  publishes source.
- Execution identity now normalizes expression children and message events, so
  production compilation accepts them.
- Sine envelopes stay finite when an attack or release product overflows the
  frame duration.
- `maac module unpack` refuses members that collide on the host filesystem and
  never replaces a destination that appears during publication.
- Library-extension objects (`library`, `wavetable`, `sample`, `instrument`,
  `voice`, `connect`, `modulate`, `control`, `preset`, instrument `node`) and
  `import` accept §4's optional string `label` instead of refusing it, and
  refuse other label values with `E_UNIT`.

### Limitations

- The Rust implementation is a foundation subset; it does not claim full
  Document, Performance, Core Audio, or Locked Render conformance.
- The built-in renderer has no raw-message adapter, so rendering message
  events fails with `E_CAPABILITY`. Module-asset stretchers, mixed external and
  core processor graphs, external inputs, events and automation, unknown
  extension semantics, live MIDI transport, GUI, and real-time or live DSP
  playback remain deferred or outside the current interfaces. Recognized
  deferred features fail explicitly with `E_CAPABILITY`.
- The normative specification remains a design draft and needs
  implementation-driven review before stabilization.
- Automated finite, non-silent sample measurements and repeatability checks do
  not establish cross-platform bit identity or human listening quality.
  Informal owner approvals of the mono and stereo showcase renders are
  recorded; a broader listening review is pending.
- This is a source-only preparation; generated binaries and rendered audio are
  not included. The starter library includes its small authored wavetable WAV.
