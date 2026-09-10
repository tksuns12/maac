# Core delay and causal feedback

`core.delay/1` implements [MaaC/1 §18.5](../MaaC-1-Specification.md#185-coredelay1)
with the graph execution rules in [§16](../MaaC-1-Specification.md#16-causality-and-graph-execution).

```maac
node echo {
  type = "core.delay/1";
  config = { channels = 1; frames = 4800; };
}
```

Both configuration fields are required positive integers. This engine supports
mono or stereo; larger positive channel counts fail with `E_CAPABILITY`.
`frames` is an integer sample count, not a time quantity. Zero-frame delays are
invalid. The single audio `in` requires exactly one connection with the same
channel count as `out`. The processor has no parameters.

The output is `y[n] = x[n - frames]`. History before reset is zero. Prepared
history storage is a bounded ring buffer; rendering allocates no delay history
per frame. Reset clears the buffer and cursor. The configured delay continues
through the explicit project tail without extending the project automatically.

For every sample the engine reads all delay outputs from history, evaluates
the same-sample graph, and then writes all current delay inputs. Connections
into an explicit delay therefore do not impose a same-sample dependency.
Their ports and channel counts are still checked. All other audio and
modulation dependencies remain in the graph, and a remaining cycle fails with
`E_ALGEBRAIC_LOOP`. Equally available nodes retain stable node-ID order;
summing inputs retain connection-ID order.

This supports feedback, including a delay connected to itself. It adds no
automatic stabilization, attenuation, clipping, or normalization. Nonfinite
arithmetic fails explicitly. A processor's reported latency alone does not
make it a causal break.

`Processor::technical_latency_frames()` reports the configured frame count for
delay and zero for the other supported core processor variants. Rendering does
not compensate this delay or crop away its initial silence.

Retained plans use `{"kind":"delay","channels":1,"frames":4800}` within the
existing plan versions. Source-free replay preserves the graph and configured
delay. Execution identity includes the channel and frame configuration.

Resource accounting reserves `channels * frames` binary64 history cells per
delay, aggregated with native reverb history under
`PlanLimits::max_production_delay_cells`. Products, aggregate sizes, and memory
allocation are checked before rendering. Per-frame work is `2 * channels`
normalized units, including silent or disconnected-from-output delays and the
whole render tail. Existing node, channel, duration, and execution limits also
apply.

The [complete example](../examples/core-delay.maac) demonstrates a feedback echo.
Acceptance covers exact frame shifts, initial silence, tail and reset behavior,
multiple delays, causal feedback, residual algebraic-loop rejection, resource
boundaries, and source-free CLI replay.
