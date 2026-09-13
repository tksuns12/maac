# L4 generic interchange vectors

This directory contains a bounded static corpus for the additive generic
version-1 interchange schema in
[`interchange.schema.json`](../../interchange.schema.json). The checker proves
canonical-byte, digest, fixed fixture-contract, closure, timing, and optional
evidence relationships within this corpus. It does not prove that the
production compiler, normalizer, dependency resolver, capability registry, or
renderer emits or verifies these objects.

The corpus contains four pre-render input chains and six locks:

- `minimal` binds one core processor to a synthetic engine contract.
- `equal-config-negative` shows that two distinct processor nodes may carry the
  same complete Config bytes and digest, and fixes a negative score origin.
- `external-ramp` binds a synthetic external processor implementation,
  descriptor, adapter, source-declared initial state, auxiliary slots, and two
  distinct imported-source slots. Its supplied normalized document declares
  the matching module, descriptor, state, and blob assets and the required
  fixture capability. The descriptor preserves a nested `label` field and
  control characters in canonical form.
- `external-transitive` changes one auxiliary artifact while keeping the other
  pre-render inputs fixed, which changes the render key.
- Three minimal lock variants use no evidence, PCM evidence, and PCM plus raw
  file evidence. Their render key is identical while their canonical lock
  bytes and transport digests differ.

The minimal PCM artifact has four exact binary32 samples: positive zero,
negative zero, the smallest positive subnormal, and `1.0`. It is supplied
static data; no rendering or numeric conversion runs in the checker. The
nonzero score/ramp frame count and other timing declarations are likewise
independently supplied expected facts, not results calculated by the checker.
For the linear `120 bpm` to `240 bpm` curve over `0q` to `4q`, the supplied
mapping is `T(q) = 2 ln(1 + q/4)` seconds. The `1q` to `3q` interval therefore
has `ceil(48000 * 2 * ln(7/5)) = 32302` frames. The zero- and negative-origin
step cases follow directly from `120 bpm = 2q/s`.

`contracts/fixture-engine.json` and
`contracts/fixture-external-descriptor.json` are synthetic corpus-only
published contracts. Their payloads are neither executable nor production
implementations. The checker validates only the explicitly declared fixture
capabilities and byte bindings; full engine, processor, adapter, state-restore,
normalization, dependency-discovery, and rendering correspondence remains a
runtime obligation.

Every canonical JSON file uses the §20 UTF-8 encoding, has no BOM and no final
newline. `SHA256SUMS` covers the exact inventory listed by `manifest.json`.
Config digests hash the complete Config envelope. Execution hashes cover the
supplied normalized typed documents. Render keys hash the complete canonical
render-input envelope and exclude `render_key` and `evidence`.

The invalid vectors exercise closed shape, literal version and canonical
decimal rules, canonical control-character escaping, ordering and identity
uniqueness, dependency closure and cross-pinning, config and render-key
digests, state-slot closure, timing bounds, channel permutation, and PCM
evidence length. The focused Python tests also mutate transport hashes and the
manifest after selected corruptions so transport self-consistency cannot stand
in for the semantic relationships.
