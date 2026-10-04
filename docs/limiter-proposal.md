# True-peak limiter: `fx.limiter/1` proposal

**Status:** accepted and implemented, 2026-10-04. The normative contract is
[production §6](production.md#6-limiter--fxlimiter1); this page keeps the
reasoning and the decisions.

## Why

The [T2 production trial](ai-production-trial.md) ranked a limiter second
among the things to build (F9, F10, F16). Without one, an AI masters to a
loudness target by stacking a glue compressor and a fast peak compressor,
then calculating a final gain. In the trial this:

- left a 16.8 dB crest factor;
- pushed the true peak to +0.05 dBTP as soon as a plucked guitar replaced the
  flute, so the stand-in had to be re-tuned.

Mastering should take one setting: "make it this loud, never above this
ceiling".

## The constraint that shapes the design

A limiter that guarantees its ceiling must see a peak before it arrives. It
therefore delays its output. MaaC already decides how that works.
[§15.1](../MaaC-1-Specification.md#151-latency-is-explicit) says:

> Every processor descriptor reports its fixed technical latency in frames
> … MaaC/1 does not add hidden delay compensation. An authoring operation may
> calculate alignment delays and insert named delay nodes; those become
> source state.

The limiter therefore declares its latency, and nothing in the engine changes.
On the master the delay is inaudible: 1.5 ms by default. A delivery that
must keep stems sample-aligned with the limited master gives those stems an
explicit `core.delay/1` of the same length. The same section already expects a
graph validator to warn when paths with different latencies recombine.

## Proposed contract

`fx.limiter/1` is a native processor of the
[production extension](production.md) (`maac.production/1`). It follows the
shared native rules: a 48 kHz engine, `config.channels` of 1 or 2, one audio
`in` and `out`, finite binary64 arithmetic, and range policy `error`.

| Field | Evaluation | Default | Allowed range |
| --- | --- | --- | --- |
| `config.channels` | Immutable | required | 1 or 2 |
| `config.lookahead` | Immutable | `1.5ms` | 0.25 through 10 ms |
| `params.gain` | Every sample | `0dB` | 0 through +24 dB (input drive) |
| `params.ceiling` | Every sample | `-1dB` | −24 through 0 dB |
| `params.release` | Every sample | `100ms` | 1 ms through 5 s |

**Latency.** The latency is `L = round(48000 × lookahead)` frames, from 12
through 480. Output frame `n` carries input frame `n − L`. The project `tail`
should be at least the lookahead, or the last `L` frames fall outside the
render.

**Processing, per frame:**

1. **Drive.** `u = x × 10^(gain/20)` for each channel.
2. **Detection.** Run the four-times Annex 2 interpolator that the delivery
   analyzer uses (`maac.truepeak.bs1770-5.annex2-4x/1`, same taps) on `u`.
   The detected level `d` is the larger of the channels' sample magnitudes and
   their interpolated magnitudes. Detection is linked across channels, so the
   stereo image never shifts. The interpolator's group delay of about six
   frames lies inside the lookahead.
3. **Required gain.** `r = min(1, 10^(ceiling/20) / d)`.
4. **Gain envelope.** A detection made `L` frames before it reaches the
   output lowers the gain along a straight line, from 1 when it is detected to
   `r` when it arrives. The applied gain is the minimum of every pending
   detection's line and a release envelope. With no constraint, the release
   envelope returns toward 1 with e-folding time `release`. The gain never
   exceeds 1.
5. **Output.** Each channel's output is `u` delayed by `L` frames, times the
   gain.

**Guarantees.** Output sample peaks never exceed the ceiling. Because the
detector also sees inter-sample peaks, the output's true peak, measured by the
delivery analyzer, stays at or within a small margin of the ceiling. Tests will
fix that margin; the target is 0.1 dB. The algorithm is deterministic.

**State.** The gain starts at 1, and the delay line and interpolator start at
zero, at the project's reset origin. Like the other native processors, state
persists through the tail.

## Options

### Latency

- **A — lookahead with declared latency (recommended).** As above: an exact
  sample-peak ceiling, a near-exact true-peak ceiling, and smooth gain. It
  fits §15.1 and needs no engine change.
- **B — zero latency.** The gain is cut at the moment a peak is detected. With
  no lookahead the cut is abrupt and audible. True peaks cannot be guaranteed,
  because the interpolator needs about six future samples.

### Drive

- **Built-in `gain` (recommended).** One node expresses "push into the
  ceiling", which is how mastering limiters are used.
- **No drive.** Authors keep a separate `core.gain/1` before the limiter.

### Home

- **`maac.production/1` (recommended).** It sits next to the EQ, compressor
  and reverb, under the same profile rules.
- **A core processor (`core.limiter/1`).** It would be usable without the
  production extension, but adding to the core is a language change.

## After it lands

- **Guide.** The [AI producer guide](ai-producer-guide.md) replaces its
  two-compressor workaround with one limiter.
- **Example.** "Late Window" uses it in place of its peak compressor. The
  delivery checks and a re-run of the trial measurements show the difference.

## Decisions

The owner accepted every recommendation on 2026-10-04:

1. **Latency:** A, lookahead with declared latency.
2. **Drive:** built-in `gain`.
3. **Defaults:** 1.5 ms lookahead, -1 dB ceiling and 100 ms release.
4. **Home:** the production extension.

## Implementation notes

- **Hold window.** The first implementation held each peak's required gain
  only from the interpolator's group delay onward. Driven noise then measured
  0.29 dB over the ceiling, because each interpolated value depends on twelve
  input frames. The contract therefore holds the gain over all twelve: the
  ramp ends when the earliest frame of the support reaches the output. The
  same noise now measures 0.011 dB over a −1 dB ceiling.
- **Residual margin.** The gain can still vary inside one support window, and
  the interpolator's negative taps can turn that into a small overshoot. The
  contract states the tested 0.1 dB bound rather than an exact guarantee.
- **Delivery margin.** Dither and 24-bit encoding lifted a −1 dB ceiling to
  −0.99999 dBTP, which fails a −1 dBTP limit. The guide sets the ceiling about
  0.1 dB under the delivery limit; "Late Window" uses −1.1 dB.
- **Result.** "Late Window" masters with one limiter (`gain = 9.5dB`,
  `ceiling = -1.1dB`, `release = 150ms`) at −14.73 LUFS and −1.10 dBTP. Its
  stand-in of two compressors and a calculated gain reached −15.58 LUFS and
  −2.07 dBTP.
