# Core fader

`core.fader/1` implements the decibel gain processor in
[MaaC/1 §18.2](../MaaC-1-Specification.md#182-coregain1-and-corefader1).

```maac
node master {
  type = "core.fader/1";
  config = { channels = 2; };
  params = { level = -6dB; };
}
```

`config.channels` is required and must be a positive integer. This engine supports
one or two channels; larger positive counts fail with `E_CAPABILITY`. The single
audio input `in` requires exactly one connection with the same channel count.
Audio output `out` has that count. There is no implicit channel conversion or
input summation. The processor has no event ports or control output.

`level` is a finite signed dB value, default `0dB`. Each frame, the fader computes
`factor = 10^(level/20)` and multiplies every input channel by that factor. Zero
dB is unity, +20 dB multiplies by ten, and -20 dB multiplies by one tenth. There
is no smoothing, clipping, normalization, or arbitrary dB range. A nonfinite
factor or output fails with `E_NONFINITE`, including when the input is silent.
The conversion and multiplication use binary64 arithmetic; there is no scaled
fallback to recover an overflowing intermediate factor, and underflow follows
the numeric implementation's ordinary rounding behavior.
The fader has no history and reports zero technical latency.

Global automation supports step and linear interpolation in dB. Exponential
interpolation of dB values is rejected, following the language's curve rules.
Top-level modulation adds dB contributions in modulation-ID order before the
conversion to linear gain; its source must be a dimensionless control output.
Parameter values, automation points, and modulation amounts require dB units in
source. The `gain` parameter belongs to `core.gain/1` and is not a fader alias.

Plans retain the processor as `{"kind":"fader","channels":2}` and store numeric
`level` values in dB. The existing plan version is selected from the composition's
features; top-level modulation uses V7. Source-free replay preserves the fader,
its automation, and its modulation. Production identity reconstruction also
preserves the dB unit. Existing graph, channel, and execution limits apply; the
processor adds no persistent DSP storage.

Execution accounting adds `128 + channels` units per rendered frame for each
fader: a conservative conversion allowance plus channel multiplications. The
charge includes silent or disconnected-from-output nodes and the entire render
tail. These are normalized resource units, not measured CPU instructions.

The [complete example](../examples/core-fader.maac) demonstrates automation and
modulation. Acceptance covers mono/stereo unity, signed dB levels, sample-exact
changes without smoothing, nonfinite failures, strict source/retained validation,
and repeatable CLI replay with atomic output failure.
