# MaaC CLI and library reference

This reference covers the MaaC CLI and Rust library for the MaaC source
language.

## Commands

| Invocation | Result |
| --- | --- |
| `maac check INPUT [--project-root ROOT]` | Resolve the bounded source bundle and validate a composition or every library export |
| `maac compile INPUT -o PLAN [--project-root ROOT]` | Resolve a composition bundle and write an independently loadable versioned JSON plan |
| `maac render PLAN -o WAV` | Validate an imported plan, then render it |
| `maac build INPUT -o WAV [--project-root ROOT]` | Resolve, compile and render a composition bundle through the same library APIs |
| `maac hash FILE` | Print the `sha256:` pin for the file's exact bytes without writing a file |

`--json` selects structured command results and diagnostics. `--force` permits
replacing an existing output. Render/build accept `--format float32` (default)
or `--format pcm16`. Exit status is zero for success and nonzero for failure.
Use the executable's `--help` for argument syntax.

`check`, `compile`, and `build` resolve hash-pinned imports and wavetable assets
before compilation. `INPUT` may be absolute or relative to the current working
directory. By default its containing directory is the project root. Pass
`--project-root ROOT` when valid project-relative dependencies cross that
directory boundary. Resolution remains local and rejects absolute dependency
paths, root escapes, symlink escapes, missing files, cycles, and hash
mismatches. `check` accepts both compositions and libraries; `compile` and
`build` require a composition.

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
| `bundle_fs::load_bundle` | Read a bounded, hash-pinned local dependency graph under an explicit project root |
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
`bundle_fs::load_bundle` and the CLI own filesystem discovery, while the
compiler receives a complete `SourceBundle`. Bundle compilation supplies the
resolved reusable-instrument context that the public single-document
`semantic::validate_source` boundary does not have. The CLI also owns paths and
atomic destination publication; export converts streamed binary64 samples to
the selected WAV encoding.

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
