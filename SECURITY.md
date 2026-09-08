# Security policy

MaaC is an experimental source-only release. The security scope includes
the source parser, semantic compiler, imported performance-plan validator, DSP
renderer, WAV exporter, and command-line file handling.

## Reporting a vulnerability

Private vulnerability reporting is enabled through GitHub Security Advisories
for this public repository. Submit a report through the [MaaC private
vulnerability reporting form](https://github.com/tksuns12/maac/security/advisories/new).

Do not put sensitive details, exploit code, credentials, or private data in a
public issue. Include the affected version or commit, the component,
reproduction steps that do not expose secrets, and the impact.

## Disclosure status

No public vulnerability disclosures are listed in this repository. The
experimental implementation has not undergone a complete security audit, and
the absence of a published disclosure is not a security guarantee. The
[capability matrix](docs/capabilities.md) documents resource bounds and
unsupported features; callers should validate untrusted plan files through the
public APIs and keep generated output under appropriate filesystem permissions.
