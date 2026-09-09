# MaaC (Music as a Code)

<img src="output/imagegen/maac-logo.png" alt="MaaC logo: a musical note between angle brackets followed by the MaaC wordmark" width="480">

MaaC (Music as a Code) is a Rust library and command-line tool that implements
MaaC/1 for checking source, compiling an inspectable performance plan, and
rendering that plan to WAV. The playable foundation targets offline 48 kHz mono
or stereo rendering with the bounded [capability set](docs/capabilities.md).

The installed executable includes [24 basic instruments](docs/basic-instruments.md):
keys, guitars, basses, drums, strings, flute, bell, pad and lead. Import
`std/basic/1.0.0` without downloading sounds or supplying a library directory.
These are synthesized interpretations, with stereo output and common controls.

The separate [acoustic guitar collection](docs/acoustic-guitars.md) supplies
three MaaC-native string instruments under `std/acoustic/1.0.0`, with bends
and vibrato. The [delivery report](docs/acoustic-delivery.md) records numerical
and installed-example checks; user listening approval remains pending. Basic
sounds and default discovery remain unchanged.

Reusable [sound libraries](docs/instruments.md) add code-authored instruments,
presets, sample-wise FM, and morphing wavetables. Import local source files with
SHA-256 pins, instantiate their instruments, and automate the exposed controls.
Compiled version 2 plans embed everything needed for offline rendering.

The [project-entrypoint guide](docs/project-entrypoint.md) specifies conventional
`main.maac` discovery and the explicit finite `--profile song` allowance for
larger projects. Existing explicit-file commands remain compatible.

The MaaC/1 language uses the `.maac` extension, and source files begin with the
canonical `maac 1;` header. The current tree is an
experimental v0.1.0 MaaC source-only release prepared for
GitHub. Build the executable locally; this repository does not promise binary,
rendered audio, or other generated release artifacts. The starter library's
small wavetable WAV is authored source data. The MaaC language specification is a
design draft, and the Rust implementation deliberately covers a smaller,
documented subset.

The experimental [native mixing and delivery implementation](docs/production.md)
adds EQ, compression, algorithmic reverb, and named master/stem deliveries under
`maac.production/1`. The graph runs at 48 kHz; `maac deliver` exports 44.1, 48,
or 96 kHz WAV with explicit encoding and dither, then analyzes the final files.
The [production example](examples/production.maac),
[delivery schema](production.schema.json), and
[bounded fixtures](production-conformance.json) describe the contract. The
[production delivery report](docs/production-delivery.md) records installed-CLI
acceptance separately from the specification fixtures.
Metering uses analyzer `maac.analysis.bs1770-5/2` and the approved single-stage
four-times Annex 2 true-peak profile. See the applicable fixture results and
historical profile comparison in the [metering evidence](docs/production-metering-evidence.md).
No professional sound-quality or full ITU/EBU conformance claim is made.

## Quick start

Install a current stable Rust toolchain. From the repository root:

```sh
cargo build --release --locked
cargo install --path . --locked
```

Discover a built-in sound and render a complete example:

```sh
maac instruments
maac instruments mellow_piano --json
maac build examples/basic/mellow_piano.maac -o piano.wav --format pcm16
maac build examples/basic/full_band.maac -o full-band.wav --format pcm16
```

Each basic example is standalone: copy its `.maac` file anywhere and use the
installed executable. The [basic instrument guide](docs/basic-instruments.md)
includes a complete first composition, pitch guidance and drum conventions.

Then check, compile, and render a composition:

```sh
maac check example.maac
maac compile example.maac -o example.performance.json
maac render example.performance.json -o example.wav
```

Render the named production example from the repository root:

```sh
maac deliver examples/production.maac --project-root . --profile song \
  --delivery release_cd --output-dir production-output
maac compile examples/production.maac --project-root . --profile song -o production.json
maac deliver production.json --profile song --delivery archive --output-dir archive-output
```

Repeat `--target ID` to select a subset; omitted targets select all. Every
selected output observes the complete graph, including external sidechains and
the declared tail. The example's illustrative loudness limits may fail: completed
WAVs and the manifest remain available, and the command returns failure. Existing
files are protected unless `--force` is supplied. See the
[CLI reference](docs/reference.md) for formats, limits, and publication behavior.

The compiler and renderer do not contact the network. Once dependencies are
cached, `--offline` can be added to Cargo commands. Build/render default to a
48 kHz float32 WAV; `--format pcm16` selects overload-rejecting PCM16 export.
Existing destinations require `--force`, and failed operations leave the
destination intact.

For a one-step build, or a PCM16 render, use:

```sh
maac build evening-window.maac -o evening-window.wav
maac build example.maac -o example.pcm16.wav --format pcm16
```

Try the shared FM bell, wavetable pad, and bass library:

```sh
maac check examples/sounds/studio.maac
maac build examples/reusable.maac -o reusable.wav
```

The [starter sound guide](examples/README.md) explains the presets, multiple
instances, and wavetable source asset. Use `maac hash FILE` to obtain a source
or wavetable pin after editing a dependency.

## Documentation

Read the [quick start](docs/quickstart.md), [authoring tutorial](docs/tutorial.md),
[CLI and library reference](docs/reference.md), and [capability matrix](docs/capabilities.md).
The [pitch guide](docs/pitch-expression.md) and [gain guide](docs/gain-expression.md)
demonstrate independent bends, swells, and fades on overlapping `core.sine/1` voices.
The [instrument pitch guide](docs/instrument-pitch.md) applies independent bends
and vibrato to reusable basic, acoustic, and custom voices.
The [instrument gain guide](docs/instrument-gain.md) applies swells and fades to
basic, acoustic, and custom instrument voices with a standalone example.
The [timbre guide](docs/timbre-expression.md) maps independent per-note color
curves through explicit custom graph sources, with a standalone filtered-oscillator
and plucked-string example.
The [pressure guide](docs/pressure-expression.md) completes all four per-note
expression kinds for explicitly opted-in graphs, with independent pressure
mappings to oscillator level and brightness in a standalone stereo example.
The [diagnostics guide](docs/diagnostics.md) explains failures, while the
[performance-plan format](docs/performance-plan.md) describes the standalone
JSON interchange artifact. [Implementation decisions](docs/implementation-decisions.md)
and the [implementation plan](docs/implementation-plan.md) record the current
technical boundary. The [instrument delivery evidence](docs/instrument-delivery.md)
and historical [verification report](docs/verification.md) separate automated
evidence from the pending listening review. The [changelog](CHANGELOG.md)
records the experimental release scope.

The full [MaaC-1 specification](MaaC-1-Specification.md),
[surface grammar](grammar.ebnf), executable [Lark grammar](grammar.lark),
[syntax-tree schema](syntax-tree.schema.json), and the asset-free
[example](example.maac) are included in the source tree. The companion
`check_spec.py` script is a syntax and selected-semantics smoke checker; it is
not a complete semantic validator, renderer, or proof of specification
conformance.

## Development

The [contribution guide](CONTRIBUTING.md) lists the local checks and the
maintainer-led contribution process. CI targets stable Rust on macOS. Python
3.10 or newer is needed for the development checks, including the source smoke
checker and acceptance runner; CI exercises Python 3.12. The checker
dependencies are pinned in `requirements-dev.txt`:

```sh
python3 -m venv .venv
source .venv/bin/activate
python3 -m pip install -r requirements-dev.txt
python3 check_production.py
python3 scripts/production_src_coefficients.py --verify
python3 scripts/acceptance.py
python3 scripts/production_acceptance.py --profile song
```

The initial release validation was performed on macOS; other platforms are
unverified. No minimum supported Rust version (MSRV) is promised.

`check_spec.py` regenerates the tracked `check-results.json`, `conformance.json`,
and `example.syntax.json` beside itself. Run it from a disposable copy if you
do not intend to update those fixtures; CI uses a disposable copy and compares
the generated results without changing the checkout.

`check_production.py` is a separate, nonmutating production-specification check
using the same pinned dependencies. It validates syntax, schema, selected
semantics, and bounded numerical fixtures. Passing it does not establish
renderer support, official ITU/EBU metering conformance, or listening quality;
the [production contract](docs/production.md) defines the separate renderer gates
and the [metering audit](docs/production-metering-evidence.md) records their current limits.

For a local disposable run:

```sh
smoke_dir="$(mktemp -d)"
cp check_spec.py grammar.lark syntax-tree.schema.json example.maac "$smoke_dir/"
python3 "$smoke_dir/check_spec.py"
```

The basic acceptance runner uses only the Python standard library after
installation and keeps generated files under `target/acceptance`. The production
runner also uses only the standard library, performs a fresh offline install,
and defaults to `--profile song`. It checks source/retained deliveries, target
selection, overwrite protection, and retained byte-identical audio after
deliberately failing limits. Runs are preserved under
`target/production-acceptance/run-*`, with the latest report at
`target/production-acceptance/results.json`. This CI gate does not download
official audio or replace the [metering audit](docs/production-metering-evidence.md).
Use `--baseline-report PATH` to compare all six example WAV hashes with an
earlier successful production report from the same platform. This optional
regression check does not impose platform-specific hashes on other systems.

The instrument delivery checks also run the installed CLI, retained-plan audio,
qualified legacy WAV comparisons, and a disposable syntax-checker copy:

```sh
python3 scripts/instrument_acceptance.py --python .venv/bin/python
```

These checks use the pinned checker packages and retain evidence under
`target/instrument-acceptance`. Fixed legacy WAV hashes are qualified to the
macOS environment recorded in the [delivery report](docs/instrument-delivery.md).

## License

Unless a file states otherwise, the original material in this repository—Rust
source, the MaaC specification, grammars, schemas, examples, tests, and
documentation—is copyright 2026 tksuns12 and licensed under the
[Apache License, Version 2.0](LICENSE). The [NOTICE](NOTICE) file records that
scope and the treatment of dependencies.

MaaC's Rust dependencies are separate works. Their copyright, attribution,
and license terms remain with the respective crates and are not replaced by
this project's Apache license. Redistributors must preserve the notices and
terms required by each dependency; consult the dependency package contents and
metadata for the applicable license information.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms and
[SECURITY.md](SECURITY.md) for the current vulnerability-reporting status.
