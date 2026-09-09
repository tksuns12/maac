# Foundation acceptance cases

These cases translate the approved plan into observable behavior. Tests are
independent of the existing Python smoke checker, whose generated files remain
unchanged.

| Boundary | Required evidence |
| --- | --- |
| Syntax | Both source files parse; spans and authored text survive; comments, strings, signed pitches and exact fractions parse; malformed input and duplicate identities fail |
| Source semantics | Unknown fields/kinds, invalid nesting, wrong references/units, invalid unused declarations, and recognized unsupported features fail explicitly |
| Musical clocks | Exact thirds and step changes; meter-boundary checks; positive and negative bar coordinates; exact ceiling scheduling |
| Pitch | Letter accidentals, key, ratio, frequency, tuning degrees including negative indices; finite positive frequencies below Nyquist |
| Per-note pitch | All three curve clocks, exact knot selection, inherited score stretch, offsets/cuts/overrides/inserts, independent overlapping voices and release holding; source/plan/audio roundtrip; unsupported receivers and malformed/over-budget curves fail |
| Expansion | Nested uses, repetition and stretch, additive transposition, nested cuts before physical offsets, stable addresses, isolated final-state overrides and inserts |
| Scheduling | Score-origin reset, invalid effective onsets, score-end note-off truncation, negative/zero gates, sub-sample collapse |
| Plan import | Unknown versions/fields, inconsistent time/frame values, duplicates, missing references, invalid parameter ranges, cycles, oversized input and collections |
| DSP | Sine/envelope samples, one-pole impulse, equal-power pan, connection-ID sum order, right-continuous automation knots, event-rate attack/release capture |
| Voices | Note-offs precede note-ons; release tails retain allocation; zero velocity retains allocation; overflow fails without stealing |
| WAV/CLI | All five documented invocation forms; float32 and PCM16 headers; overload rejection; overwrite protection; destination survives all failures |
| Examples | 48 / 182 notes; 816,000 / 1,968,000 stereo frames; finite and non-silent samples; unchanged musical content |
| Repeatability | Repeat renders produce identical PCM in the tested executable/environment |
| Release | macOS toolchain, formatting, Clippy, all tests, offline installed CLI, documented listening status |

Finite/non-silent measurements and PCM repeatability are automated evidence.
They do not establish a human listening review or cross-platform bit identity.
