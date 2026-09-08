# MaaC CLI and library reference

This reference covers the MaaC CLI and Rust library for the MaaC source
language.

## Commands

| Invocation | Result |
| --- | --- |
| `maac check INPUT` | Parse, validate and resolve the bounded performance |
| `maac compile INPUT -o PLAN` | Write an independently loadable versioned JSON plan |
| `maac render PLAN -o WAV` | Validate an imported plan, then render it |
| `maac build INPUT -o WAV` | Compile and render through the same library APIs |

`--json` selects structured command results and diagnostics. `--force` permits
replacing an existing output. Render/build accept `--format float32` (default)
or `--format pcm16`. Exit status is zero for success and nonzero for failure.
Use the executable's `--help` for argument syntax.

Source and plan inputs are read with a 4 MiB bound. Failed writes do not publish
partial destinations. Compilation validates source and schedules; rendering can
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
| `semantic::validate_source` | Source schema, units, references and capability checks |
| `compiler::check` | Complete bounded compilation validation without returning the plan |
| `compiler::compile` | Resolve a parsed document into a validated `plan::Plan` |
| `plan::Plan::from_json` | Load and independently validate untrusted plan bytes |
| `maac::load_plan` | Root convenience wrapper for bounded plan-byte loading and validation |
| `plan::Plan::validate` | Validate an in-memory plan |
| `plan::Plan::to_json` | Validate and serialize a bounded plan artifact |
| `dsp::render` | Stream binary64 output frames to a fallible callback |
| `dsp::DspEngine` | Prepare, reset and render an immutable plan repeatedly |
| `export::write_wav` | Stream WAV samples to a caller-provided `Write + Seek` sink |
| `export::render_wav_to_path` | Render and atomically publish a WAV destination |
| `export::WavFormat` | Select `Float32` or `Pcm16` conversion |

The compiler and DSP do not perform filesystem or network operations. The CLI
handles paths and atomic destination publication; export converts streamed
binary64 samples to the selected WAV encoding.

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

The AST retains authored source, object/field/value spans, exact numeric values,
and typed value variants. It is not an editing transaction engine. The plan
retains resolved events and source mappings; it is not a substitute for the
authored source. See the [format reference](performance-plan.md) and
[diagnostics guide](diagnostics.md).
