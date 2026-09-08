# Quick start

From the repository root, build and install the MaaC executable:

```sh
cargo build --release --locked
cargo install --path . --locked
```

Cargo may download dependencies during installation. `Cargo.lock` pins the
resolved dependency versions. Once dependencies are cached, add `--offline` to
Cargo commands. The installed compiler and renderer do not use the network.

Check a composition, compile an inspectable plan, and render it:

```sh
maac check example.maac
maac compile example.maac -o example.performance.json
maac render example.performance.json -o example.wav
```

The same compiler and renderer are available as one operation:

```sh
maac build evening-window.maac -o evening-window.wav
maac build example.maac -o example.pcm16.wav --format pcm16
```

New WAV files use 48 kHz and float32 samples by default. PCM16 export rejects
samples outside `[-1,1]`. Neither encoding normalizes, limits or dithers. Existing
destinations require `--force`; a failed operation leaves the destination intact.

`example.wav` contains 48 notes in 17 seconds including its tail.
`evening-window.wav` contains 182 notes in 41 seconds including its tail. Open the
WAV files in an audio player for listening. Automated output measurements and
listening status are recorded separately in the release verification report.

Use `--json` for machine-readable command results and diagnostics:

```sh
maac check example.maac --json
```

Read the [authoring tutorial](tutorial.md), [capability matrix](capabilities.md),
and [diagnostics guide](diagnostics.md) before using features beyond these examples.
The [full specification](../ScoreIR-1-Specification.md) defines the language;
foundation support is explicitly narrower than its conformance profiles.
