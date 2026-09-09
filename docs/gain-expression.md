# Per-note gain expression

Attach an `expression` with `kind = gain` to a note and reference a curve
whose values are dimensionless, nonnegative amplitude multipliers. Each
overlapping voice evaluates its own curve. A note without gain expression
uses gain 1. Values above 1 are valid; gain is not capped at 1.

```maac
curve swell {
  clock = normalized;
  points = [(0, 0, linear), (1/2, 1, linear), (1, 0, step)];
}
// Inside a note:
expression dynamics { kind = gain; curve = &swell; }
```

[The example](../examples/gain-expression.maac) overlaps two one-second
notes on one `core.sine/1` node. The first swells from 0 to 1 and fades to 0
while bending pitch. The second rises exponentially from 1/8 to 1/2 and
holds gain 1/2 through its release. It uses modest velocity and node level.
The 120 bpm score lasts 1.5 seconds; its 200 ms tail accommodates the
150 ms release. Expected output is mono, 48 kHz, **81,600 frames (1.7 seconds)**,
including the remaining silence at the end of the tail.

From the repository root, with `maac` on PATH:

```sh
maac check examples/gain-expression.maac
maac build examples/gain-expression.maac -o gain-expression.wav
maac compile examples/gain-expression.maac -o gain-expression.plan.json
maac render gain-expression.plan.json -o gain-expression-from-plan.wav
```

Use fresh output paths; these commands do not request overwriting files.
For a source checkout, replace `maac` with `cargo run --bin maac --`.
Direct build and compile-then-render must produce identical audio.

## Clocks and interpolation

| Clock | Position units | Meaning |
| --- | --- | --- |
| `normalized` | Dimensionless, from 0 to 1 | Fraction of the scheduled gate |
| `seconds` | `s` or `ms` | Physical time since scheduled onset |
| `score` | `q` | Affine progress through the final score duration |

Start at position zero, use strictly increasing positions, and finish with
shape `step`. A normalized curve must end at 1. Gain supports `step`,
`linear`, and `exponential`. Step and linear segments allow zero; both
endpoints of every exponential segment must be strictly positive.
Interpolation uses amplitude multipliers, not decibels. At a knot, the
new point applies. Outside the points, endpoint values hold.

For scheduled onset frame `N`, note-off frame `M`, and sample frame `n`,
let `elapsed = clamp(n - N, 0, M - N)`. The evaluation positions are:

- normalized: `elapsed / (M - N)`;
- seconds: `elapsed / sample_rate`;
- score: `elapsed / (M - N) * (final_score_off_q - final_score_on_q)`.

Physical offsets affect the scheduled gate. The score extent is retained
before physical project-end truncation; `N` and `M` reflect actual scheduling
and truncation. Score expression follows that extent affinely across the
actual gate, rather than following the global tempo clock.

Inherited placement and nested-use stretch factors multiply score-curve
positions only. Seconds and normalized positions stay unchanged. A `dur`
occurrence override changes the gate without additionally scaling points.
Inserted notes already use placement-local final coordinates, so their
expression point scale is 1. During release, the gate-end gain holds.
These clock rules also apply to [pitch expression](pitch-expression.md).

## Acceptance contract and limits

- `core.sine/1` and reusable mono/stereo instruments receive gain expression,
  including `std/basic/1.0.0`, `std/acoustic/1.0.0`, and custom graphs. See the
  [instrument gain guide](instrument-gain.md) for voice placement and limits.
  Pressure and timbre remain unsupported. Instrument pitch remains
  `E_CAPABILITY`, including when combined with zero gain.
- One pitch and one gain expression may coexist on a `core.sine/1` note. Expression
  child IDs and their order do not affect the result; duplicate kinds fail.
- Each `core.sine/1` voice emits `velocity * gain * envelope * level * sin(phase)`.
  Zero gain silences its contribution but preserves phase updates, voice
  allocation, and the ordinary note/release lifecycle.
- There is no automatic normalization or limiting. Gain above 1 can
  overload output; the existing PCM export overload check still applies.
- Ordinary notes without expression must preserve their previous output.
- Plan v1/v2 carry an optional note `gain_expression` object with `clock`
  and `points` (`position`, `gain`, `shape`), using canonical rationals.
  `None` is omitted, preserving old JSON. Older readers reject plans
  containing the new field. Rust `EventKind::Note` literals need
  `gain_expression: None` when gain expression is absent.
- Gain and pitch share `ExpressionClock`; the `PitchExpressionClock`
  type alias preserves the previous clock name for Rust callers.

The governing rules are in [MaaC-1](../MaaC-1-Specification.md), sections
6.1, 8.1, 9, 11, 12, and 18.7. This guide states the acceptance contract;
it does not itself establish test results or listening approval.
The [delivery report](gain-expression-delivery.md) records the automated
checks and installed-CLI evidence separately.
