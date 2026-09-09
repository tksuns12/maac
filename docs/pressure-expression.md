# Per-note pressure expression

This contract extends reusable instrument graphs with an explicit per-voice
pressure signal, following MaaC-1 section 8.1. Instrument authors choose its
meaning through existing connections and modulation. Pressure is independent
of velocity, gain, pitch, and timbre; it has no automatic amplitude mapping.

## Source and mapping

```maac
node touch { type = "synth.pressure/1"; }
modulate dynamics {
  from = &touch:out;
  to = &osc.params.level;
  depth = 1/2;
}
```

The processor is voice-only, stateless, inputless, and parameterless, with one
mono `out`. Omit `config`; even an empty configuration is rejected. An empty
`params` record is allowed. Its output may feed ordinary mono audio inputs or
existing sample-rate modulation targets. Shared graphs and event-rate modulation
targets remain invalid.

An instrument opts in by declaring at least one `synth.pressure/1` node in its
voice graph, including an unused node. `InstrumentProgram::supports_pressure()`
derives this capability from the graph, with no serialized capability flag.
Every pressure node in a voice receives the same per-frame value. Timbre and
pressure sources advertise their respective capabilities independently.
`core.sine/1`, graphs without a pressure source, and the unchanged frozen
basic/acoustic libraries reject pressure expressions with `E_CAPABILITY`, even
for zero curves, zero velocity, or zero gain.

Public controls and automation supply parameter baselines. Modulations add
`depth * source_value` in modulation-ID order, with signed depth in the target's
unit. Final bounds are checked after all contributions. There is no implicit
clipping or remapping. For example, level baseline `1/4` plus depth `1/2` maps
pressure from zero to one onto level `1/4` through `3/4`, before other modulation.
Authors must keep the combined controls and modulation within target bounds.

## Curves and playback

```maac
curve touch_shape {
  clock = normalized;
  points = [(0, 0, linear), (1, 1, step)];
}
// Inside a note:
expression touch_change { kind = pressure; curve = &touch_shape; }
```

Values are exact dimensionless rationals in `[0,1]`; absent pressure is zero.
One expression of each of pitch, gain, timbre, and pressure may coexist on a
note. Expression child order does not change modulation order.

Pressure follows the [timbre contract](timbre-expression.md) for normalized,
seconds, and score clocks, effective gates, inherited score stretch, offsets,
duration overrides, and inserted notes. Positions start at zero and strictly
increase; normalized curves end at one; the final point has shape `step`.
Step and linear segments allow zero. Exponential segments require strictly
positive exact endpoints and use the existing robust scalar interpolation.
Knots are right-continuous and endpoint values hold outside the curve.

Pressure is evaluated once per voice per frame, including the first rendered
sample. The gate-end value holds through release. Overlapping voices have
independent curves. Oscillator phase, filter state, envelopes, and plucked-string
history continue across pressure changes. Zero gain or velocity does not skip
expression evaluation, graph processing, bounds checks, allocation, or ordinary
voice retirement. Shared effects process the completed voice sum as before.

## Retained plans and limits

Note events gain optional `pressure_expression` with `clock` and `points`.
Each point has canonical rational `position` and `value`, and `shape`. Unknown
fields are rejected. The public Rust types are `PressureExpression` and
`PressureExpressionPoint`. Rust `EventKind::Note` literals must supply
`pressure_expression: None` when absent; the public runtime `note_on` signature
stays unchanged.

Absent pressure is omitted from JSON, preserving existing plan wire shapes.
Graph instruments still require plan version 2; this extension does not bump
the version. Older readers reject the new payload or processor identity.
Source compilation and retained-plan validation enforce the same capabilities
and bounds.

Automation and all four expression kinds share the existing 65,536-point limit,
including expanded notes. Source compilation counts expanded points before
cloning curves. The 4,096-bit rational limit and object limits remain in force.
Each pressure-bearing voice frame adds `17 + ceil(log2(point_count))` execution
work units, additive with pitch, gain, and timbre. The conservative duration
includes gate and maximum automated release, capped at render end, even for
silent voices. Source nodes and edges incur ordinary graph resource charges;
multiple source nodes reuse one evaluated pressure value. These limits are not
a wall-clock performance guarantee.

## Standalone example

[The example](../examples/pressure-expression.maac) uses three overlapping notes
and all three pressure clocks. Its mappings are explicit:

- Oscillator level is `1/16 + (1/8 * pressure)`, bounded by `1/16` and `3/16`.
- Filter cutoff is `800Hz + (1200Hz * pressure) + (2400Hz * timbre)`,
  bounded by 800 Hz and 4400 Hz.

The first note combines pitch, gain, timbre, and pressure, with timbre falling
while pressure rises. Other notes omit timbre and receive zero from that source.
The seconds curve uses positive exponential endpoints; the score curve combines
step and linear segments. Source values hold at gate end through the 300 ms
release, covered by the 400 ms project tail. There are no imports or assets.

```sh
maac check examples/pressure-expression.maac
maac build examples/pressure-expression.maac -o pressure-expression.wav
maac compile examples/pressure-expression.maac -o pressure-expression.plan.json
maac render pressure-expression.plan.json -o pressure-expression-from-plan.wav
```

Expected output is 91,200 stereo frames at 48 kHz: 1.5 seconds of score plus
0.4 seconds of tail. Numerical checks do not establish listening approval.

The [delivery report](pressure-expression-delivery.md) records automated
validation, independent review, and remaining limits.
