# Per-note gain on reusable instruments

Use the existing `expression { kind = gain; curve = &curve_id; }` syntax
on notes targeting `std/basic/1.0.0`, `std/acoustic/1.0.0`, or custom mono/stereo
instruments. Each overlapping note follows its own nonnegative amplitude
curve. No additional instrument control or library edit is needed.

```maac
curve swell {
  clock = normalized;
  points = [(0, 0, linear), (1/2, 3/4, linear), (1, 0, step)];
}
// Inside a note targeting an instrument:
expression dynamics { kind = gain; curve = &swell; }
```

The [gain guide](gain-expression.md) defines the unchanged normalized, seconds,
and score clocks, step/linear/exponential interpolation, and pattern expansion
rules. Gain above 1 is allowed without normalization or limiting; allow output
headroom. [Per-note pitch](instrument-pitch.md) can coexist independently with gain on
these instruments and on `core.sine/1`; zero gain still preserves pitch validation.

## Voice behavior

The renderer evaluates the complete voice graph, computes its existing
`sample[channel] * amplitude * velocity` contribution, and then multiplies
that contribution by the note's gain on every channel. Voices sum afterward,
and shared effects process the sum. Notes without gain retain the existing
arithmetic and audio path.

Zero gain silences that voice's new contribution but does not skip DSP,
envelopes, voice allocation, or ordinary retirement. In a plucked string it
neither retriggers nor refills the string: when gain returns, the existing,
already-decaying string becomes audible. Gain evaluated at the gate end holds
through release. Shared filter/effect history can remain audible after a
voice reaches zero gain or retires. Use the normal project tail for that history.

## Try the example and retain a plan

[The standalone example](../examples/instrument-gain.maac) overlaps two notes
on a mellow piano and two on an acoustic nylon guitar. It includes a swell,
an exponential rise, a temporary guitar mute, and a score-clock fade. Both
imports are embedded, so the source can be copied outside this checkout.
Low levels and velocities leave modest headroom.

From the repository root, with an executable supporting instrument gain on PATH:

```sh
maac check examples/instrument-gain.maac
maac build examples/instrument-gain.maac -o instrument-gain.wav
maac compile examples/instrument-gain.maac -o instrument-gain.plan.json
maac render instrument-gain.plan.json -o instrument-gain-from-plan.wav
```

Use fresh output paths. For a source checkout, replace `maac` with
`cargo run --bin maac --`. The 120 bpm, three-quarter-note score lasts 1.5
seconds; its 400 ms tail includes the 300 ms releases. Expected output is
stereo at 48 kHz, **91,200 frames (1.9 seconds)**. Direct build and
compile-then-render must produce identical audio in the same executable and
environment. These commands are verification steps, not a claim of recorded
results or listening approval.

Keep or copy the JSON plan and run the `render` command later with a compatible
executable; embedded instrument programs make it independent of the original
source files. This extension reuses the existing optional `gain_expression`
field: no new wire fields, plan version, frozen library bytes, defaults, or
controls. The public Rust `InstrumentRuntime::note_on` API remains unchanged.

## Resource limits

For each active instrument voice frame carrying gain, validation adds
`17 + ceil(log2(point_count))` normalized execution-work units to the existing
`max_execution_work` budget: 1 point costs 17, 2 points cost 18, and 65,536
points cost 33 units per frame. The conservative lifetime includes the gate
and maximum automated release, capped at render end, even for zero gain.
This is resource accounting, not a wall-clock performance guarantee.

The combined 65,536-point automation/expression limit and 4,096-bit rational
limit remain unchanged, including after expansion. Source and retained-plan
validation enforce the same receiver and resource constraints; the
[performance-plan guide](performance-plan.md) describes the existing wire format.

The [delivery report](instrument-gain-delivery.md) records automated playback,
installed CLI verification, and the remaining validation limits.

Custom voice graphs declaring `synth.timbre/1` may also attach one independent
[timbre expression](timbre-expression.md) alongside pitch and gain. Declaring
`synth.pressure/1` independently permits [pressure](pressure-expression.md),
so all four kinds may coexist when both sources are present. Frozen basic
and acoustic graphs do not opt in to timbre or pressure. Automation, pitch, gain, timbre, and pressure share the
65,536-point limit; each attached instrument expression contributes its own
`17 + ceil(log2(point_count))` work charge per conservative active voice frame.
