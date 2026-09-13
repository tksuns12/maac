# Reset-correct WAV range validation

Historical implementation evidence date: 2026-09-13.

This evidence covers the offline WAV excerpt boundary described in
[`render-range.md`](render-range.md). Fixtures are generated or rendered from
repository examples; no playback, recording, or listening assessment is part
of this slice.

## Historical implementation log — 2026-09-13

The pre-implementation red check was:

```text
cargo test --test render_range --locked --offline
```

It failed three range-focused tests because the binary did not yet accept the
range flags and the range result behavior was not implemented.

The focused post-implementation check passed all six tests:

```text
cargo test --test render_range --locked --offline
# test result: ok. 6 passed; 0 failed
```

Those tests cover reset-origin stateful delay history, the version 7 audio and
modulation path, Float32 bit-pattern and PCM16 payload slices, explicit full
and empty intervals, paired and out-of-bounds errors, PCM16 validation before
and after a crop, destination preservation, the non-Render library helper
guard, and the help surface.

The dedicated range fixtures cover these artifact generations:

- legacy: `examples/core-delay.maac`, including stateful delay prehistory and
  exact Float32 and PCM16 payload slices
- version 3: `examples/tempo-ramps.maac`, including explicit full and empty
  intervals and preservation of the default result shape
- version 7: `examples/core-modulation.maac`, including the audio and
  modulation path and an exact stereo Float32 payload slice

Versions 4 through 6 use the same artifact-view and WAV export structure that
was reviewed for this change, but they do not have individual dedicated range
fixtures in this slice.

Additional checks completed before the repository-wide gates:

```text
cargo check --locked --offline
# passed
cargo test --lib export::tests --locked --offline
# test result: ok. 1 passed; 0 failed
cargo fmt --all -- --check
# passed
cargo build --release --locked --offline
# Finished `release` profile
```

The release-built executable was exercised in an isolated temporary directory
`/tmp/maac-range-release-final.z5p4Qo`:

```text
target/release/maac --json compile examples/core-delay.maac -o .../delay.json
target/release/maac --json render .../delay.json -o .../full.wav
target/release/maac --json render .../delay.json -o .../crop.wav \
  --start-frame 2400 --end-frame 7200
```

The full result reported `frames: 38400`; the excerpt reported
`frames: 4800`, `start_frame: 2400`, and `end_frame: 7200`. Parsing each RIFF
`data` chunk and comparing the excerpt payload with bytes
`2400 * 4 .. 7200 * 4` of the full Float32 payload returned:

```text
release_payload_cmp=ok bytes=19200
```

The historical current-tree Rust gates were recovered and observed
independently after an earlier full-test process was interrupted without a
recoverable result. No code changed during that recovery. Raw output and status
files are preserved under `/tmp/scoreir-excerpt-verification.OiFC4J`.

```text
cargo fmt --all -- --check
# exit 0
cargo clippy --all-targets --locked --offline -- -D warnings
# exit 0; Finished `dev` profile in 1.23s
cargo test --locked --offline
# exit 0; 1,005 passed, 0 failed, 3 ignored across 134 result lines
```

The full test gate reported three built-in ignored checks, kept distinct from
failures or incomplete checks:

- `production_analysis::tests::ebu_3341_prescribed_loudness_tones_1_through_5`:
  full-duration EBU acceptance, run explicitly in release mode
- `production_analysis::tests::ebu_official_corpus_audit`: requires the
  external official EBU v05 corpus with the audit's hash-pinned provenance
- `production_analysis::tests::itu_official_loudness_corpus_audit`: requires
  the private ITU BS.2217 corpus through `MAAC_ITU_CORPUS`

The native implementation executor owned the code, focused red/green tests,
release build, and release CLI payload comparison. The recovery executor owned
the final Rust gates above and this evidence update. A separate read-only
review cleared `src/export.rs`, `src/cli.rs`, and `tests/render_range.rs` before
the final gates.

## Fresh current-tree verification — 2026-09-14

The current shared tree was rechecked after the historical log above. The full
Rust test gate and the range integration test both passed:

```text
cargo test --locked --offline
# exit 0; 1,005 passed, 0 failed, 3 ignored
# render_range: 6 passed, 0 failed
cargo fmt --all -- --check
# exit 0
cargo clippy --all-targets --locked --offline -- -D warnings
# exit 0
cargo build --release --locked --offline
# exit 0
```

The release-built executable was exercised in the isolated temporary directory
`/tmp/maac-range-resume.m62I9R`. Comparing the selected RIFF payload with the
corresponding full-render Float32 bytes returned:

```text
full_frames=38400 crop_frames=4800
release_payload_cmp=ok bytes=19200
```

No actual playback, recording, listening, or DAW acceptance was performed. No
separate standalone Python corpus or hosted-CI run was launched beyond the
Rust gate; the log records the Rust contract suites that were included. This
slice does not claim DSP seek optimization, realtime behavior, production
delivery range support, or completion of end-to-end MaaC production.
