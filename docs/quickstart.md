# Quick start

From the repository root, build and install the MaaC executable:

```sh
cargo build --release --locked
cargo install --path . --locked
```

Cargo may download dependencies during installation. `Cargo.lock` pins the
resolved dependency versions. Once dependencies are cached, add `--offline` to
Cargo commands. The installed compiler and renderer do not use the network.

The executable includes 24 synthesized basic instruments. List them, inspect a
sound, then build an audition or a short ensemble:

```sh
maac instruments
maac instruments mellow_piano --json
maac build examples/basic/mellow_piano.maac -o piano.wav --format pcm16
maac build examples/basic/full_band.maac -o full-band.wav --format pcm16
```

Each example imports `std/basic/1.0.0` from the executable. Its `.maac` file can
be copied to an empty directory and rendered without library files or samples.
The [basic instrument guide](basic-instruments.md) includes a complete source
to save as your first composition, common controls, and fixed-tuning drum usage.

The separate [acoustic guitar guide](acoustic-guitars.md) includes a complete
string-model composition and expressive controls, verified with the installed
executable. Exact library discovery is:

```sh
maac instruments --libraries
maac instruments --library std/acoustic/1.0.0
maac instruments nylon_guitar --library std/acoustic/1.0.0 --json
```

Without `--library`, discovery continues to select the basic collection.

For a project with a conventional `main.maac`, use these
[entrypoint commands](project-entrypoint.md), verified through the
[installed CLI](project-entrypoint-delivery.md):

```sh
maac check
maac compile -o song.performance.json
maac build . -o song.wav --format pcm16
```

Omitted input means the current directory's `main.maac`; a directory input
means that directory's `main.maac`. No parent or recursive search occurs.
`--project-root` controls containment without changing which entry is selected.
Explicit filenames remain valid, and output paths remain relative to the shell's
working directory.

A larger native composition can explicitly select the finite song work budget:

```sh
maac check compositions/soldier-of-fortune --profile song
maac build compositions/soldier-of-fortune --profile song -o soldier.wav --format pcm16
maac compile compositions/soldier-of-fortune --profile song -o soldier.performance.json
maac render soldier.performance.json --profile song -o retained.wav --format pcm16
```

`song` raises only execution work from the default 500 million to 10 billion
units. All other bounds stay unchanged. `check` includes the chosen budget;
a retained large plan needs the explicit profile again when rendered. The
source or plan cannot grant itself a higher allowance. `render` and `hash`
continue to require explicit file inputs.

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
The [full specification](../MaaC-1-Specification.md) defines the language;
foundation support is explicitly narrower than its conformance profiles.
