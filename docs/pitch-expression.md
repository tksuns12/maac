# Per-note pitch expression

Attach an `expression` with `kind = pitch` to a note and reference a curve
whose values are cents. Each voice evaluates its own curve independently,
including when notes overlap on the same `core.sine/1` node.

[The example](../examples/pitch-expression.maac) slides A3 up one octave
over a three-quarter-note gate while a later E5 slides down 700 cents.
It uses modest levels, mono output, and a 300 ms project tail so the
200 ms release can finish.

```maac
curve rise {
  clock = normalized;
  points = [(0, 0ct, linear), (1, 1200ct, step)];
}
// Inside a note:
expression bend { kind = pitch; curve = &rise; }
```

From the repository root, with `maac` on PATH:

```sh
maac check examples/pitch-expression.maac
maac build examples/pitch-expression.maac -o pitch-expression.wav
maac compile examples/pitch-expression.maac -o pitch-expression.plan.json
maac render pitch-expression.plan.json -o pitch-expression-from-plan.wav
```

Use fresh output paths; these commands do not request overwriting files.
For a source checkout, replace `maac` with `cargo run --bin maac --`.

## Clocks and interpolation

| Clock | Position units | Meaning |
| --- | --- | --- |
| `normalized` | Dimensionless, from 0 to 1 | Fraction of the scheduled gate |
| `seconds` | `s` or `ms` | Physical time since scheduled onset |
| `score` | `q` | Affine progress through the final score duration |

Start at position zero, use strictly increasing positions, and finish with
shape `step`. A normalized curve must end at 1. Pitch supports `step` and
`linear`; interpolation is in cents. `exponential` is invalid for cents.
At a knot, the new point applies. Outside the points, endpoint values hold.
During release, the value evaluated at note-off holds.

For scheduled onset frame `N`, note-off frame `M`, and sample frame `n`,
let `elapsed = clamp(n - N, 0, M - N)`. The evaluation positions are:

- normalized: `elapsed / (M - N)`;
- seconds: `elapsed / sample_rate`;
- score: `elapsed / (M - N) * (final_score_off_q - final_score_on_q)`.

Physical offsets affect the scheduled gate. The score extent is retained
before physical project-end truncation; `N` and `M` reflect actual scheduling
and truncation. Score-clock expression therefore follows that extent
affinely across the actual gate, rather than following the global tempo clock.

Inherited placement and nested-use stretch factors multiply score-curve
positions only. Seconds and normalized positions stay unchanged. A `dur`
occurrence override changes the gate without additionally scaling points.
Inserted notes already use placement-local final coordinates, so their
expression point scale is 1.

## Acceptance contract and limits

- Only pitch expression on `core.sine/1` is supported in this slice.
  Gain, pressure, timbre, and expression on custom or other built-in
  instruments remain `E_CAPABILITY` errors. This is narrower than the full
  specification's `core.sine/1` expression contract.
- Expression changes the note's resolved base frequency by
  `2^(cents / 1200)`. Initial expression applies at note-on, each overlapping
  voice retains its own expression, and release holds the gate-end pitch.
- Pitch at or above Nyquist, including the bend, must fail explicitly.
- Direct build and compile-then-render must produce identical audio;
  ordinary notes without expression must preserve their previous output.
- Plan v1/v2 carry an optional note `pitch_expression` object with `clock`
  and `points` (`position`, `cents`, `shape`), using canonical rationals.
  Absent expression is omitted, preserving legacy JSON. Older readers reject
  expression-bearing plans. Rust `EventKind::Note` literals need
  `pitch_expression: None` when expression is absent.

The governing rules are in [MaaC-1](../MaaC-1-Specification.md),
sections 6.1, 8.1, 12, and 18.7. This guide states the acceptance contract;
it does not itself establish test results or listening approval.
The [delivery report](pitch-expression-delivery.md) records the automated
checks and installed-CLI evidence separately.
