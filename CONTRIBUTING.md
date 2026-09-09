# Contributing to MaaC

MaaC is a maintainer-led experimental project. Keep changes focused on the
documented foundation scope, explain behavior changes in the pull request, and
update the relevant public documentation when an interface or limitation
changes.

## Repository scope

Keep repository changes directly related to the MaaC language or tool, including
implementation, tests, specifications, documentation, functional examples, and
supporting project configuration. Keep personal compositions in the local `compositions/`
directory, which is excluded from version control. Do not force-add that
material or include it in repository or remote changes.

## Development setup

Use a current stable Rust toolchain and Python 3.10 or newer for the development
checks. CI exercises Python 3.12 and stable Rust on macOS. From the repository
root, create an isolated Python environment, install the checker dependencies,
fetch the locked Cargo dependencies, and run the same checks used by CI:

```sh
python3 -m venv .venv
source .venv/bin/activate
python3 -m pip install -r requirements-dev.txt
cargo fetch --locked
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
cargo build --release --locked --offline
python3 check_production.py
python3 scripts/production_src_coefficients.py --verify
python3 scripts/acceptance.py
python3 scripts/production_acceptance.py --profile song
```

`check_spec.py` regenerates the tracked `check-results.json`, `conformance.json`,
and `example.syntax.json` beside itself. Run it from a disposable copy if you
do not intend to update those fixtures; CI uses a disposable copy and compares
the generated results without changing the checkout. The basic acceptance
runner is standard-library-only and writes generated artifacts below
`target/acceptance`.

The production acceptance runner also uses only the Python standard library.
After the normal Cargo cache preparation, it performs a fresh offline install
and exercises source and retained-plan deliveries under the `song` profile
(the default). It checks decoded artifacts, source-free replay, target subset
and order independence, overwrite protection, and retained byte-identical WAVs
from a separate source copy with deliberately failing loudness limits. It does
not download official fixtures or qualify the true-peak estimator. Each run
retains its install, artifacts, and report under
`target/production-acceptance/run-*`; the latest report is
`target/production-acceptance/results.json`. See the
[production delivery report](docs/production-delivery.md) for observed evidence.
An optional `--baseline-report PATH` checks that all six example WAVs retain
their hashes against an earlier successful report from the same platform.
Every run checks the current analyzer/profile identities and complete 4×
true-peak filter flushing, even without a baseline report.

Cargo may need network access during `cargo fetch --locked` to populate the
local cache. The later checks use `--offline` and must run only after that fetch
has completed.

The initial release validation was performed on macOS; other platforms are
unverified. No minimum supported Rust version (MSRV) is promised.

For a local disposable smoke-check run:

```sh
smoke_dir="$(mktemp -d)"
cp check_spec.py grammar.lark syntax-tree.schema.json example.maac "$smoke_dir/"
python3 "$smoke_dir/check_spec.py"
```

Run `python3 check_production.py` from the repository root after installing the
pinned checker dependencies above. This separate checker does not regenerate
tracked files. It validates the [production specification](docs/production.md)
example, hash-pinned schema, selected semantics, and bounded numerical fixtures.
Keep those artifacts consistent when changing the contract. These checks cover
specification arithmetic; Rust renderer tests and installed-CLI acceptance
provide separate implementation evidence. Applicable official ITU/EBU fixtures,
independent SRC verification, and listening acceptance remain distinct gates.
The [metering audit](docs/production-metering-evidence.md) records applicable
current 4× profile fixtures and the historical 16× profile's failed gate.

## Contributions

Open a change with a clear description of the user-visible or technical
behavior, the tests that cover it, and any remaining limitation. Keep source
files, examples, schemas, and documentation internally consistent. Changes to
the normative specification should identify the affected section and explain
whether the Rust foundation implements that behavior.

By submitting a contribution for inclusion, you agree that it may be
distributed under the Apache License, Version 2.0, under the terms in
[LICENSE](LICENSE). MaaC does not require a separate contributor license
agreement. Submit only material you have the right to contribute, and retain
third-party notices when incorporating external material.

Please do not include secrets, private data, generated build artifacts, or
unrelated formatting changes in a contribution. For suspected security issues,
follow [SECURITY.md](SECURITY.md) rather than posting sensitive details in a
public issue.
