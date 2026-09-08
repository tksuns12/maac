# MaaC CLI and library reference

This reference covers the MaaC CLI and Rust library for the MaaC source
language.

## Commands

| Invocation | Result |
| --- | --- |
| `maac check [INPUT] [--project-root ROOT] [--profile default\|song]` | Resolve and budget-check a composition, or validate every library export |
| `maac compile [INPUT] -o PLAN [--project-root ROOT] [--profile default\|song]` | Resolve a composition bundle and write an independently loadable versioned JSON plan |
| `maac render PLAN -o WAV [--profile default\|song]` | Validate an explicit plan file under the caller's profile, then render it |
| `maac build [INPUT] -o WAV [--project-root ROOT] [--profile default\|song]` | Resolve, compile and render a composition bundle through the same selected limits |
| `maac instruments [NAME]` | List the built-in catalog, or show one instrument with controls, musical guidance and runnable usage |
| `maac instruments [NAME] --library ID` | Select an exact built-in version for catalog listing or named detail |
| `maac instruments --libraries` | List embedded library identities, reserved source paths and hashes |
| `maac hash FILE` | Print the `sha256:` pin for the file's exact bytes without writing a file |

`--json` selects structured command results and diagnostics. `--force` permits
replacing an existing output. Render/build accept `--format float32` (default)
or `--format pcm16`. Exit status is zero for success and nonzero for failure.
Use the executable's `--help` for argument syntax.

Optional/directory entry and execution-profile forms above implement the
[project-entrypoint extension](project-entrypoint.md); see its
[validation evidence](project-entrypoint-delivery.md). For source commands, omitted `INPUT` selects
`cwd/main.maac`, and an existing directory selects `DIR/main.maac`. There is no
recursive or ancestor search. `-o` remains required where shown, and relative
outputs remain relative to the process cwd. Explicit filenames retain their
existing behavior; `render`/`hash` do not discover `main.maac`.

`--profile` is per-command on check/compile/build/render, not a language
conformance declaration. Default work remains 500,000,000; explicit `song`
permits at most 10,000,000,000 work units with every other bound unchanged.
Composition `check` includes this budget validation. Large retained plans need
explicit song selection on render; source/plan data cannot select an allowance.

`check`, `compile`, and `build` resolve hash-pinned imports and wavetable assets
before compilation. Exact `builtin` imports resolve from the executable, with
no sound-library directory or asset download. `INPUT` may be absolute or
relative to the current working directory. For explicit files, the existing
canonical-file-parent default root is preserved. Omitted/directory forms retain
the selected directory as root before resolving any `main.maac` symlink; an
outside target fails unless an explicit root admits it. Entry-symlink imports
retain their canonical-target declaring-file interpretation. Pass
`--project-root ROOT` when valid project-relative dependencies cross that
directory boundary. Resolution remains local and rejects absolute dependency
paths, root escapes, symlink escapes, missing files, cycles, and hash
mismatches. `check` accepts both compositions and libraries; `compile` and
`build` require a composition.

Built-in imports use `import basic { builtin = "std/basic/1.0.0"; }`. This form
cannot include `path` or `hash`; unavailable versions fail explicitly. Discovery
uses `maac instruments --json` for `catalog`, or `maac instruments NAME --json`
for `instrument`, within the structured command result. Catalog records include
`library`, `source_path`, `source_hash` and `instruments`. Each instrument has
`name`, `family`, `description`, `channels`, `guidance`, `controls` and runnable
`usage`. Control metadata includes `unit`, `rate`, exact-string rational
`default`/`min`/`max`, and `min_open`/`max_open`.

Version-selected discovery supports `std/acoustic/1.0.0` separately from the
default `std/basic/1.0.0`; the [acoustic guide](acoustic-guitars.md) records its
current validation status. `--libraries` conflicts with both a positional name
and `--library`. Unknown versions and names produce `E_REFERENCE`, without
fallback. Explicit selection adds optional top-level JSON `library`; library
listing adds `libraries: LibraryInfo[]`, with `command: "instruments"` and
`input: "@builtin"`. Unused optional fields are omitted. Default basic JSON
results remain unchanged.

The loader pins an open project-root directory and uses contained relative
opens for each source and asset. It verifies regular-file type and reads bytes
from the same handle, preserving containment if path components change during
resolution. Contained relative and absolute symlinks remain supported.

Source, plan, and `hash` inputs use a 4 MiB per-file bound. In human mode,
`maac hash FILE` writes one copy-ready `sha256:` line. With `--json`, the same
value is the `digest` field. Failed output writes do not publish partial
destinations. Compilation validates source and schedules; rendering can
additionally fail for runtime voice capacity, nonfinite DSP results, export
overload, or I/O errors.

Float32 export rounds each binary64 sample to IEEE float32 and rejects nonfinite
conversion. PCM16 first rejects samples outside `[-1,1]`, then uses
`round(sample * 32767)` with halfway cases away from zero. Thus `-1` maps to
`-32767` and `+1` to `32767`; this symmetric convention leaves `-32768` unused.
It does not dither or change the gain of the rendered signal.

## Library boundaries

| API | Responsibility |
| --- | --- |
| `syntax::parse` / `maac::parse` | Parse a string into a source-preserving `Document` |
| `syntax::parse_file` | Bounded filesystem convenience wrapper |
| `bundle::SourceBundle` / `maac::SourceBundle` | Own project-relative MaaC source text and asset bytes for deterministic resolution |
| `bundle_fs::load_bundle` | Read a bounded local dependency graph under an explicit project root and resolve embedded built-ins |
| `stdlib::catalog` | Return `Result<Catalog, Diagnostics>` for the embedded collection, including source identity and exact control metadata |
| `stdlib::instrument` | Return `Result<InstrumentInfo, Diagnostics>` for one exact instrument name |
| `stdlib::libraries` | Return `Vec<LibraryInfo>` with exact `library`, `source_path`, and `source_hash` strings |
| `stdlib::catalog_for(library)` | Return `Result<Catalog, Diagnostics>` for one exact library identity |
| `stdlib::instrument_in(library, name)` | Return `Result<InstrumentInfo, Diagnostics>` for an exact library/name pair |
| `semantic::validate_source` | Validate source schema, units, references, and the built-in processor profile for a parsed document |
| `compiler::check` / `maac::check` | Validate one parsed document; imports are rejected because they have not been resolved |
| `compiler::compile` / `maac::compile` | Compile one parsed document; imports are rejected because they have not been resolved |
| `compiler::check_bundle` / `maac::check_bundle` | Resolve in-memory sources and validate a composition or every library export |
| `compiler::compile_bundle` / `maac::compile_bundle` | Resolve and compile a composition bundle into a self-contained version 2 plan |
| `plan::Plan::from_json` | Load and independently validate untrusted plan bytes |
| `maac::load_plan` | Root convenience wrapper for bounded plan-byte loading and validation |
| `plan::Plan::validate` | Validate an in-memory plan |
| `plan::Plan::to_json` | Validate and serialize a bounded plan artifact |
| `dsp::render` | Stream binary64 output frames to a fallible callback |
| `dsp::DspEngine` | Prepare, reset and render an immutable plan repeatedly |
| `export::write_wav` | Stream WAV samples to a caller-provided `Write + Seek` sink |
| `export::render_wav_to_path` | Render and atomically publish a WAV destination |
| `export::WavFormat` | Select `Float32` or `Pcm16` conversion |

The compiler and DSP do not perform filesystem or network operations.
`stdlib::catalog()` and `stdlib::instrument(name)` derive public control
metadata from the embedded source definition. `catalog()` and `instrument(name)` retain their basic-library
defaults; selected-library APIs add discovery without changing those defaults.
The existing `stdlib::lookup(id) -> Option<BuiltinSource>` signature is unchanged.
Built-in imports still require
the bundle APIs: single parsed-document `check`/`compile` reject unresolved
imports, even when the imported source is embedded.
`bundle_fs::load_bundle` and the CLI own filesystem discovery, while the
compiler receives a complete `SourceBundle`. Bundle compilation supplies the
resolved reusable-instrument context that the public single-document
`semantic::validate_source` boundary does not have. The CLI also owns paths and
atomic destination publication; export converts streamed binary64 samples to
the selected WAV encoding.

The explicit resource extension adds `PlanLimits::song()` and consistent
`_with_limits` compiler, plan encode/decode, DSP, and WAV-export APIs. Existing
wrappers and generic Serde plan decoding keep default limits. No profile or
limits object is serialized into a plan. See the
[explicit limits contract](project-entrypoint.md#explicit-rust-limits-across-every-boundary)
for the agreed boundaries; chosen limits must propagate through all nested
validation, not be replaced by default wrappers midway through an operation.

For an explicitly authorized larger composition, the agreed API sequence is:

```rust
let limits = maac::plan::PlanLimits::song();
let plan = maac::compile_bundle_with_limits(&bundle, &limits)?;
let bytes = plan.to_json_with_limits(&limits)?;
let retained = maac::load_plan_with_limits(&bytes, &limits)?;
maac::render_with_limits(&retained, &limits, |frame| {
    assert!(frame.iter().all(|sample| sample.is_finite()));
    Ok(())
})?;
```

The selected-limit APIs passed the integrated regression suite. Existing
examples below intentionally use unchanged default wrappers.

For example, with a `source: &str` already available:

```rust
let document = maac::parse(source)?;
let plan = maac::compiler::compile(&document)?;
let bytes = plan.to_json()?;
let imported = maac::plan::Plan::from_json(&bytes)?;

let mut frames = 0_u64;
maac::dsp::render(&imported, |frame| {
    assert!(frame.iter().all(|sample| sample.is_finite()));
    frames += 1;
    Ok(())
})?;
assert_eq!(frames, imported.output.total_frames);
```

The callback receives one mono or stereo frame and can return an error to stop
rendering. It should stream to its consumer rather than accumulating a long
render in memory. Every new render starts from reset processor state.

For a composition containing only built-in imports, no dependency maps are
needed:

```rust
let bundle = maac::SourceBundle::new("song.maac", composition_source);
maac::check_bundle(&bundle)?;
let plan = maac::compile_bundle(&bundle)?;
let catalog = maac::stdlib::catalog()?;
let piano = maac::stdlib::instrument("mellow_piano")?;
```

The resolver adds the reserved identity `@builtin/std/basic/1.0.0.maac` and its
actual source hash. Embedded sources consume the existing bundle limits.
Version 2 plans contain library graphs, resolved noise seeds and provenance;
retained-plan rendering does not consult the catalog or require original
source files. Exact released built-in source bytes are frozen; updated sounds
require a new library version. See [basic instruments](basic-instruments.md).

For reusable sounds, supply the entry text, imported library text, and WAV
bytes already available to the host. Keys match the project-relative paths
declared in the sources, and their embedded pins must match the supplied bytes:

```rust
let mut bundle = maac::SourceBundle::new("reusable.maac", composition_source);
bundle.sources.insert("sounds/studio.maac".into(), library_source.into());
bundle.assets.insert("sounds/colors.wav".into(), wav_bytes);
maac::check_bundle(&bundle)?;
let plan = maac::compile_bundle(&bundle)?;
let standalone_json = plan.to_json()?;
```

`SourceBundle` also accepts a library as its entry for `check_bundle`.
`compile_bundle` requires a composition. The [starter sources](../examples/README.md)
provide a complete composition, library, and pinned wavetable asset.

The AST retains authored source, object/field/value spans, exact numeric values,
and typed value variants. It is not an editing transaction engine. The plan
retains resolved events and source mappings; it is not a substitute for the
authored source. See the [format reference](performance-plan.md) and
[diagnostics guide](diagnostics.md).
