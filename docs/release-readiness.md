# MaaC release readiness

This document separates local MaaC source-only preparation from decisions that
remain before release publication. MaaC is the project and `maac` is the CLI;
ScoreIR/1 remains the source-language name, `.maac` is the MaaC file extension,
and `maac 1;` is the canonical document header; legacy `scoreir 1;` is
rejected. The public [tksuns12/maac repository](https://github.com/tksuns12/maac)
has been created, the local `origin` is configured to its HTTPS URL, and private
vulnerability reporting is enabled. A parentless initial publication snapshot
is being prepared locally with GitHub no-reply metadata; no commit has been
pushed, and no release tag or hosted deployment is asserted here.

## Prepared locally

The working tree contains the Apache-2.0 project metadata and release support
files: the `maac` package/library/binary metadata in `Cargo.toml` and
`Cargo.lock`, `LICENSE`, `NOTICE`, `CHANGELOG.md`, `CONTRIBUTING.md`,
`SECURITY.md`, pinned `requirements-dev.txt`, the renamed acceptance runner,
and the macOS stable-Rust workflow. The user confirmed original ownership of
the project material and `tksuns12` as the copyright owner. The source-only
scope includes the MaaC logo and generation prompt under `output/imagegen` and
excludes generated binaries, WAV files, and other build artifacts.

The fresh renamed gate, installed acceptance, and disposable snapshot identity
are recorded in the [verification report](verification.md). The 53-file
manifest includes the release additions, final MaaC documentation, and
canonical `maac 1;` source/header updates while excluding the two evidence
reports from their own identity. The prepared initial snapshot includes both
evidence reports and the two `output/imagegen` assets.

## Publication steps still pending

- The initial publication commit uses the authenticated GitHub no-reply
  identity for both author and committer. The original three-commit history is
  preserved only on a local backup branch and will not be pushed.
- Decide whether and when to create the `v0.1.0` tag and release entry. The
  changelog intentionally remains an unreleased experimental preparation.
- Push the prepared initial snapshot to the selected public destination in a
  later publication step; no push is asserted here.
- Run and inspect hosted GitHub CI after a destination exists. The workflow is
  prepared locally, but no hosted check has been observed.

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

The normative ScoreIR specification remains a design draft, the MaaC Rust
implementation is an explicitly narrower foundation subset, and the listening
review is pending. The automated evidence is tied to the documented macOS
stable-Rust environment and tested executable. It does not establish
cross-platform bit identity, hosted CI status, or human listening quality.
