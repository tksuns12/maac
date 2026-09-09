# Per-note timbre expression

This contract extends reusable instrument graphs with an explicit per-voice
timbre signal. It does not assign an automatic meaning such as brightness to
every instrument. Instrument authors choose the mapping using existing graph
connections and modulation.

## Source and receiver contract

Declare a voice-only source node and map its output to a sample-rate parameter:

```maac
node color { type = "synth.timbre/1"; }
modulate brightness {
  from = &color:out;
  to = &filter.params.cutoff;
  depth = 6000Hz;
}
```

The processor has no inputs, configuration, or parameters and has one mono
`out`. Omit `config`; even an empty configuration is rejected, matching the
existing parameterless configuration contract. An empty `params` record is
allowed. Its output may feed ordinary mono audio connections or modulation,
like an ADSR or LFO signal; this permits authors to transform the signal inside
the graph. It cannot appear in a shared graph.

An instrument opts into timbre by declaring at least one `synth.timbre/1` node
in its voice graph. All such nodes in a voice emit that voice's timbre value.
Presence is the capability declaration, even if a node is not connected; it
does not guarantee an audible effect. Capability is derived from the embedded
program, not trusted from a separate serialized flag.

Attach the existing note expression syntax to an opted-in instrument:

```maac
curve shade {
  clock = normalized;
  points = [(0, 0, linear), (1, 1, step)];
}
// Inside a note:
expression color_change { kind = timbre; curve = &shade; }
```

Notes without timbre expression use zero. `core.sine/1` and instruments without
a timbre source reject timbre expression with `E_CAPABILITY`, including zero
curves and silent notes. The frozen `std/basic/1.0.0` and `std/acoustic/1.0.0`
libraries retain their bytes and do not acquire implicit mappings. Pressure
remains unsupported.

## Mapping, timing, and state

Existing parameter resolution stays in force. A public control supplies the
parameter baseline, including its instance settings and current automation.
Modulations then add `depth * source_value` in modulation-ID order. Depth has
the target parameter's unit and may be negative. Thus a timbre mapping produces
`baseline + depth * timbre`, plus any other authored modulation contributions.
Final parameter bounds are checked after the contributions are combined.
There is no implicit clipping, normalization, or remapping. Event-rate targets
remain invalid modulation targets; shared parameters cannot receive a voice's
signal through a cross-graph edge.

For example, a cutoff baseline of 1000 Hz and depth of 6000 Hz maps timbre
0 to 1000 Hz and timbre 1 to 7000 Hz before other modulation. A wavetable
position baseline of zero and depth of one maps the same curve across its
position range. The author remains responsible for valid combined values when
controls and other modulators also change the target.

Values are exact dimensionless rationals in `[0, 1]`. Curves use the existing
normalized, seconds, and score clocks and the same scheduling, inherited
score-stretch, offset, and duration-override rules as
[pitch](pitch-expression.md) and [gain](gain-expression.md). Positions start
at zero and strictly increase; normalized curves end at one; the last point
has shape `step`. Step and linear segments allow zero. Exponential segments
require strictly positive exact endpoints, using the gain evaluator's robust
exponential interpolation behavior. Exact knot selection is right-continuous.

Timbre is evaluated once per voice per frame. Its initial value applies at the
first rendered sample, and its gate-end value holds through release. Overlapping
voices retain independent curves. One pitch, one gain, and one timbre expression
may coexist; expression child order does not determine mapping order.
Oscillators, filters, envelopes, and plucked strings keep their existing state.
Zero gain or velocity does not bypass timbre evaluation, graph work, parameter
validation, voice allocation, or ordinary retirement. Shared effects continue
to process the sum of completed voice contributions.

## Plans and resource limits

Note events gain an optional `timbre_expression` object containing `clock` and
`points`; each point has canonical rational `position` and `value`, plus
`shape`. Unknown fields are rejected. Absent timbre is omitted from JSON, and
plans without the extension preserve their existing wire shape. No plan-version
bump is introduced; graph instruments still require version 2. Older readers
reject the new payload or processor identity. Rust `EventKind::Note` literals
must supply `timbre_expression: None` when absent. The public
`InstrumentRuntime::note_on` signature stays unchanged.

Automation, pitch, gain, and timbre share the existing combined 65,536-point
limit, including after expansion. The 4,096-bit rational limit remains in
force. Source compilation counts expanded expression points before cloning
curve data. Source and retained plans enforce the same capability and limits.

Each timbre-bearing instrument voice frame adds
`17 + ceil(log2(point_count))` normalized execution-work units, additive with
pitch and gain. The conservative window includes the gate and maximum automated
release, capped at render end, including silent voices. Timbre source nodes and
their connections also consume the ordinary graph node, edge, state, and work
budgets. Multiple source nodes reuse one evaluated timbre value per voice.
This accounting is not a wall-clock performance guarantee.

## Standalone example

[The example](../examples/timbre-expression.maac) contains two inline custom
instruments and no external dependencies. One maps normalized timbre to a
filtered oscillator's cutoff: public brightness baseline 800 Hz plus
4200 Hz times timbre. The other maps a positive exponential seconds-clock curve
to plucked-string damping: `3/4 - (1/2 * timbre)`. Rising timbre reduces the
string's additional high-frequency damping; this is not a claim of monotonic
combined-loop brightness or decay. See the [pluck contract](plucked-string.md).
Two notes overlap on each instrument, with independent timbre curves. A second
oscillator note uses score-clock timbre, so timbre itself covers all three clocks.
The first oscillator note also uses
a score-clock 50-cent bend and a gain swell no greater than `1/2`.

Each mono voice sum is panned to stereo, then the two instrument outputs feed
an explicit master sum. Three quarter notes at 120 BPM plus a 400 ms tail give
an expected **91,200 stereo frames at 48 kHz (1.9 seconds)**. Both instruments
use 300 ms release, leaving 100 ms after the final release.

```sh
maac check examples/timbre-expression.maac
maac build examples/timbre-expression.maac -o timbre-expression.wav
maac compile examples/timbre-expression.maac -o timbre-expression.plan.json
maac render timbre-expression.plan.json -o timbre-expression-from-plan.wav
```

Direct build and retained-plan render should produce identical samples in the
same executable/environment. These commands describe the validation path;
this guide does not itself establish execution results or listening approval.

The [delivery report](timbre-expression-delivery.md) records the automated
checks, independent review, and remaining validation limits.
