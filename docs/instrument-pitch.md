# Per-note pitch on reusable instruments

Use the existing `expression { kind = pitch; curve = &curve_id; }` syntax on
notes targeting `std/basic/1.0.0`, `std/acoustic/1.0.0`, or custom mono/stereo
instruments. Overlapping notes bend independently. No library edit or new
instrument control is needed; `core.sine/1` continues to support the same curves.

```maac
curve bend {
  clock = normalized;
  points = [(0, 0ct, linear), (1, 200ct, step)];
}
// Inside a note targeting an instrument:
expression slide { kind = pitch; curve = &bend; }
```

The [pitch guide](pitch-expression.md) defines the unchanged normalized, seconds,
and score clocks, pattern transformations, and step/linear interpolation in
cents. Exponential pitch interpolation is invalid. Initial pitch applies at
note-on; the value evaluated at the gate end holds throughout release. Vibrato
can be approximated by alternating cents points with linear segments.

## Frequency and state

Each voice evaluates `expressed_base_hz = resolved_note_hz * 2^(cents / 1200)`.
This base must be finite, positive, and strictly below Nyquist, including the
initial value and reachable curve domain and gate end. This is the existing
base-note frequency constraint: a down-ratio cannot make an invalid base valid.
Source checking, compilation, and retained-plan validation enforce this curve
constraint, but cannot promise that arbitrary live graph controls remain valid.

Oscillator and wavetable nodes use
`f = expressed_base_hz * ratio + frequency`, with live controls and modulation,
including frequency modulation, incorporated in their parameters. Their signed
frequency range remains −24,000…24,000 Hz inclusive. Plucked-string nodes use
`f = expressed_base_hz * ratio`, with no frequency offset, and retain their
20…4000 Hz inclusive range. Effective node frequency failures are checked at
render time after live controls and modulation, even at zero gain or velocity.
No clipping, silent fallback, or ratio adjustment repairs an invalid value.

Bends preserve oscillator phase, wavetable state, and plucked-string ring
pointers and history. They do not retrigger or refill the string. Valid abrupt
changes can still produce audible artifacts. Noise nodes continue to ignore
note pitch. Each voice keeps its own pitch while shared effects process the
sum of voice contributions.

One pitch expression and one [gain expression](instrument-gain.md) may coexist
independently on a note. Gain multiplies the complete voice contribution before
summation and shared effects; zero gain does not bypass DSP or frequency checks.
Notes without expressions retain their prior audio path and output. The public
Rust `InstrumentRuntime::note_on` API remains unchanged.

## Standalone example and retained plan

[The example](../examples/instrument-pitch.maac) overlaps mellow piano and
acoustic nylon guitar notes with independent bends and piecewise-linear vibrato.
It covers all three clocks and combines pitch with gain on one piano note.
Both imports are embedded; levels and velocities are modest.

From the repository root, with an executable supporting instrument pitch on PATH:

```sh
maac check examples/instrument-pitch.maac
maac build examples/instrument-pitch.maac -o instrument-pitch.wav
maac compile examples/instrument-pitch.maac -o instrument-pitch.plan.json
maac render instrument-pitch.plan.json -o instrument-pitch-from-plan.wav
```

Use fresh output paths. For a source checkout, replace `maac` with
`cargo run --bin maac --`. The three-quarter-note score at 120 bpm lasts
1.5 seconds; its 400 ms tail includes the 300 ms releases. Expected output is
stereo at 48 kHz, **91,200 frames (1.9 seconds)**. Direct build and retained-plan
rendering must produce identical audio in the same executable and environment.
These are verification instructions, not recorded results or listening approval.

The retained JSON plan embeds instrument programs and can be rendered without
its original source files by a compatible executable. This receiver extension
reuses the existing optional `pitch_expression` field. Source syntax, wire
fields, plan versions, frozen library bytes, defaults, and controls are unchanged.
See the [performance-plan guide](performance-plan.md) for the wire format.

## Resource limits

For each conservative active instrument voice frame carrying pitch, validation
adds `17 + ceil(log2(point_count))` normalized execution-work units to the
existing `max_execution_work` budget. One point costs 17, two points cost 18,
and 65,536 points cost 33 units per frame. This charge is additive with gain.
The conservative lifetime includes the gate and maximum automated release,
capped at render end, including silent voices. It is accounting, not a wall-clock
performance guarantee.

The combined 65,536-point automation/expression limit and 4,096-bit rational
limit remain unchanged, including after expansion. Source and retained-plan
validation enforce the same receiver and resource constraints.

The [delivery report](instrument-pitch-delivery.md) records automated checks,
installed CLI evidence, and remaining validation limits.

Custom voice graphs declaring `synth.timbre/1` may also attach one independent
[timbre expression](timbre-expression.md) alongside pitch and gain. Frozen basic
and acoustic graphs do not opt in. Automation, pitch, gain, and timbre share the
65,536-point limit; each attached instrument expression contributes its own
`17 + ceil(log2(point_count))` work charge per conservative active voice frame.
