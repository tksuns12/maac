# MaaC release readiness

This document records the publication of MaaC and what its evidence does
and does not cover. It is a publication-status record, not the feature
roadmap; the [language specification and conformance plan](language-specification-plan.md#current-priorities)
owns active priorities. MaaC is the project and `maac` is the CLI;
MaaC/1 is the source language, `.maac` is its file extension, and `maac 1;` is
the canonical document header. Noncanonical language headers are rejected.

## Published

- **0.1.0, 2026-10-08.** The source is public in the
  [tksuns12/maac repository](https://github.com/tksuns12/maac), and the
  `v0.1.0` tag marks the release commit. The [changelog](../CHANGELOG.md)
  entry lists its contents.
- **Hosted CI passes.** The macOS release gate passed on GitHub for the first
  time on 2026-10-08 (run 37710710473) after fixes for the newer stable
  toolchain's clippy lints: formatting, clippy, the full test suite, the
  release build, the Python checks, and the installed acceptance runner with
  its baseline hashes.
- **Identity.** Every published commit uses the GitHub no-reply identity for
  author and committer. The original three-commit pre-publication history is
  kept only on a local backup branch and is not published.
- Private vulnerability reporting is enabled on the repository.

## Prepared for publication

The tree contains the Apache-2.0 project metadata and release support files:
the `maac` package/library/binary metadata in `Cargo.toml` and `Cargo.lock`,
`LICENSE`, `NOTICE`, `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md`,
pinned `requirements-dev.txt`, the acceptance runners, and the macOS
stable-Rust workflow. The user confirmed original ownership of the project
material and `tksuns12` as the copyright owner. The source-only scope
includes the MaaC logo and generation prompt under `output/imagegen` and
excludes generated binaries, WAV renders, and other build artifacts; small
audio fixtures and example sounds are authored source data.

The [verification report](verification.md) records the earlier gates and the
initial snapshot identity.

## Publication review findings and limits

The local publication review covered three reachable commits and 48 reachable
blobs. Its available heuristic scans found no credential or key matches. No
dedicated `gitleaks`, `trufflehog`, `detect-secrets`, `semgrep`, or
`git-secrets` scanner was installed, so this is not a complete security audit
or a clean-audit claim; custom, encoded, encrypted, short, or otherwise unusual
secrets could be missed.

The review verified license metadata for all 45 registry packages represented
by the locked Rust dependency archives and manifests, including the two
platform archives obtained from the official crates.io archive endpoint. It
also inspected the seven pinned Python packages used by CI: six MIT packages
and `typing-extensions` under PSF-2.0. No dependency source is vendored. A
future source bundle or binary redistribution must preserve each dependency's
required notices and exact terms, including any Unicode-3.0 or LLVM-exception
terms. The generic project `NOTICE` does not replace package-level notices.

The review found no license incompatibility in the inspected metadata. This
evidence does not replace legal review of a future bundle, generated artifact,
or additional dependency set.

## Remaining release risk

- The normative MaaC specification remains a design draft, and the Rust
  implementation is an explicitly narrower foundation subset.
- No one has approved the built-in instruments by ear. The owner declined to
  act as the listening gate for `std/studio`, and the `std/acoustic` review
  is still pending; their documents say so.
- The evidence comes from macOS on Apple silicon, locally and in hosted CI.
  It does not establish bit-identical output on other platforms or
  toolchains.
- CI installs the latest stable Rust, so a new clippy lint can fail a
  previously green tree.
