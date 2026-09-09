# Contributing to MaaC

MaaC is a maintainer-led experimental project. Keep changes focused on the
documented foundation scope, explain behavior changes in the pull request, and
update the relevant public documentation when an interface or limitation
changes.

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
python3 scripts/acceptance.py
```

`check_spec.py` regenerates the tracked `check-results.json`, `conformance.json`,
and `example.syntax.json` beside itself. Run it from a disposable copy if you
do not intend to update those fixtures; CI uses a disposable copy and compares
the generated results without changing the checkout. The acceptance runner is
standard-library-only and writes generated artifacts below `target/acceptance`.

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
specified but unimplemented behavior; future Rust renderer tests, applicable
official ITU/EBU fixtures, independent SRC verification, and listening acceptance
remain separate requirements.

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
