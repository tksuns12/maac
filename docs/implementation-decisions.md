# Implementation decisions and specification questions

The ScoreIR specification remains the normative language document. These notes
identify places where implementation needs an explicit interpretation or a
future specification clarification. They are not a claim of complete language
conformance.

## Project end and physical offsets

Pattern/use/placement `cut` boundaries truncate musical gates before physical
offsets, as section 9 states. At project end, the foundation applies the section 6
effective-time formula and then truncates the effective release:

`off_seconds = min(T(final_score_off) + release_offset, T(score.end))`.

The plan retains `final_score_off` before this project-end truncation. For a
120-bpm score ending at `1q`, a note with musical end `2q` and release offset
`-250ms` therefore releases at 0.5 seconds. Clamping its score coordinate first
would incorrectly release it at 0.25 seconds. The specification should state this
distinction between repetition cuts and project-end effective truncation directly.

## Pan range policy

Section 18.3 declares both a pan range of `[-1,1]` and a clamp policy. The
foundation preserves finite raw numeric pan values and clamps the evaluated
parameter to that range. It uses the declared error policies for sine parameters
and one-pole cutoff. A future descriptor schema should distinguish accepted raw
values from the post-policy range explicitly.

## Derived plan and numeric fidelity

The existing syntax-tree JSON is not the performance-plan format. Plan versioning
is independent of source-language versioning. Source/timing quantities remain
exact rationals; resolved pitch is binary64 because tuning and cents operations
generally produce irrational frequencies. This separates exact scheduling from
the DSP numeric representation without claiming exact acoustic arithmetic.

## Single-input ports

Only `core.sum/1` explicitly permits empty audio input. The foundation requires a
connection to `core.pan/1` and `core.onepole/1` single audio inputs. The specification
should spell out `zero_default` for each reference processor descriptor rather
than leaving it implicit in prose.
