# Core reference noise

`core.noise/1` implements the counter-derived reference noise in
[MaaC/1 §18.11](../MaaC-1-Specification.md#1811-corenoise1).

```maac
node texture {
  type = "core.noise/1";
  config = { channels = 2; seed = 18446744073709551615; };
}
```

`channels` is required and positive. This engine supports mono or stereo;
larger positive channel counts fail with `E_CAPABILITY`. The optional `seed`
accepts the complete unsigned 64-bit range, including zero. Its default is the
project seed, whose default is zero. Noise has no inputs or parameters; its
audio `out` has the configured channel count. Route it through gain or fader
for amplitude control.

For every zero-based engine frame and channel, SHA-256 hashes the exact bytes:

1. `maac-noise-1` followed by a NUL byte;
2. the seed as unsigned 64-bit little-endian;
3. the full node ASCII ID followed by a NUL byte;
4. the frame as unsigned 64-bit little-endian;
5. the channel as unsigned 32-bit little-endian.

The first eight digest bytes are read as an unsigned little-endian integer and
shifted right by 11. For the resulting integer `r`, the sample is
`2 * (r / 2^53) - 1`, in `[-1, 1)`. This is the specified reference generator,
separate from instrument graph noise. There is no shared random stream.
Changing the seed or node ID changes the sequence; adding an unrelated track
does not. Engine frame numbering continues through the render tail and starts
at zero after reset, independent of the project's score origin.

The immutable SHA-256 prefix is prepared once. Rendering clones that fixed
hash state and appends the frame/channel counter without allocating a new
message buffer. Noise has no history and zero technical latency.

Plans retain `{"kind":"noise","channels":2,"seed":18446744073709551615}`
within the existing plan versions. The seed is resolved before retention, so
source-free rendering has no dependency on a project-seed fallback. Production
execution identity preserves the effective seed and sound-relevant node ID.

For a node ID of `L` ASCII bytes, the full hash message contains `L + 34`
bytes. Resource accounting charges `256 * channels * ceil((L + 43) / 64)`
normalized work units per frame, including SHA-256 padding. This conservative
charge includes the full hash even though the runtime prepares its prefix.
It covers every declared noise node, silent routing, and the entire render
tail. Existing execution, node, identifier, and duration limits apply. The
hash state requires no delay-history allocation.

The [complete example](../examples/core-noise.maac) demonstrates seeded stereo
noise. Acceptance includes independent SHA-256 known-answer vectors, full-width
seeds, channel separation, reset and retained replay, unrelated-node independence,
strict source/plan validation, and resource boundaries.
