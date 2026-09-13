# Conformance evidence index

[`index.json`](index.json) is a static, repository-local index for the seven
existing L1--L5 conformance suites. It binds each native manifest by its raw
file SHA-256 and binds every native source, vector, asset, or contract payload
by a separate raw file SHA-256. The index does not change any native manifest.

The fixed suite identities and their native manifests are:

| ID | Native schema | Kind |
| --- | --- | --- |
| `l1` | `maac.conformance.identity-edit/2` | `static_corpus` |
| `l2` | `maac.l2.timing-corpus/1` | `runtime_suite` |
| `l3-inputs` | `maac.l3.input-contract-corpus/1` | `runtime_suite` |
| `l3-pan` | `maac.l3.pan-contract-corpus/1` | `runtime_suite` |
| `l3-tuning` | `maac.l3.tuning-contract-corpus/1` | `runtime_suite` |
| `l4` | `maac.conformance.generic-interchange/1` | `static_corpus` |
| `l5` | `maac.l5.core-audio-reference/1` | `runtime_suite` |

Run the stdlib-only index checker from the repository root:

```sh
python3 scripts/check_conformance_index.py --repo-root . --index conformance/index.json
```

The checker validates the closed index shape, exact native IDs and schemas,
fixture inventory, raw digests, byte counts from the L1/L4 file maps, L5 source,
asset, reference, and derivation-script pins, and repository-relative path
confinement. It rejects traversal, absolute paths,
URLs, and symlinks before reading indexed files. Recorded `check.argv`
commands are metadata only: the checker never executes them and reports
`runtime checks NOT RUN`. Runtime suite results remain the responsibility of
their recorded Rust test commands; this index makes no runtime, profile, or
full conformance claim.

The L5 native manifest uses the exact discriminator pair
`format: "maac.l5.core-audio-reference"` and integer `version: 1`; the index's
`maac.l5.core-audio-reference/1` schema identity is a closed index binding for
that pair. Its separate static reference verifier checks the fixed source,
asset, interval, and provenance derivation without compiling or rendering:

```sh
python3 scripts/check_l5_reference.py --corpus conformance/l5 --verify
```

The L5 runtime gate is a separate Rust harness and is recorded as index
metadata only:

```sh
cargo test --locked --offline --test l5_core_audio
```

The index checker never runs either recipe, and reports runtime checks as not
run. Static reference validation and runtime observation are separate evidence
claims.
