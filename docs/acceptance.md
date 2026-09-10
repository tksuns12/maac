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
| Per-note gain | Step/linear/exponential amplitude curves on all clocks; simultaneous pitch and gain; zero-gain phase/capacity preservation; release holding; extreme positive exponential endpoints; combined point budgets and source-free replay |
| Expansion | Nested uses, repetition and stretch, additive transposition, nested cuts before physical offsets, stable addresses, isolated final-state overrides and inserts |
| Scheduling | Score-origin reset, invalid effective onsets, score-end note-off truncation, negative/zero gates, sub-sample collapse |
| Arranged audio | Sliced mono/stereo rate playback, reverse, fractional phase, both overlapping fade shapes, empty sampled intervals, tail admission, reset, mixed graph routing and exact source-free replay; forged bounds, numerical loss and resource exhaustion fail explicitly |
| Warp-rate audio | Ordered musical anchors, piecewise source mapping through constant/step/ramp tempo, fractional starts, right-owned boundaries, future tempo during tails, natural-duration fades, mixed graph and retained delivery; malformed anchors, forbidden fields, forged bounds and precision/budget failures reject explicitly |
| Core modulation | Typed control outputs, constant automation and control chains, ID-ordered additive contributions and final range policy, all four LFO waves, score/seconds clocks and their tail behavior; note-on/note-off capture and bounded release work; reset-control automation, cycles, wrong ports, nonfinite results and precision/resource failures reject explicitly; source-free V7 replay and named delivery |
| Top-level reset modulation | Frame-zero source automation/chains, reset controls initialized before internal capture, authored non-reset controls, held phase with per-frame range validation, deterministic replay and one-time preparation work |
| Plan import | Unknown versions/fields, inconsistent time/frame values, duplicates, missing references, invalid parameter ranges, cycles, oversized input and collections |
| DSP | Sine/envelope samples, one-pole impulse, equal-power pan, connection-ID sum order, right-continuous automation knots, event-rate attack/release capture |
| Voices | Note-offs precede note-ons; release tails retain allocation; zero velocity retains allocation; overflow fails without stealing |
| Internal ADSR modulation | Initial per-note and pre-release source snapshots, ID-ordered capture, held event values, no double advancement, conservative preview/release work, source-free replay; stage restrictions and cycles reject |
| Internal phase modulation | Voice oscillator/wavetable/LFO onset capture, inclusive cycle endpoints, ordered phase/ADSR chains, held phase initialization, independent voices, reset replay, preview work bounds, source-free replay |
| Internal reset modulation | Shared-LFO reset capture from authored controls and silent input, ordered capture chains, no preview advancement, atomic reset failure, per-instance preview work, source-free replay |
| WAV/CLI | All five documented invocation forms; float32 and PCM16 headers; overload rejection; overwrite protection; destination survives all failures |
| Examples | 48 / 182 notes; 816,000 / 1,968,000 stereo frames; finite and non-silent samples; unchanged musical content |
| Repeatability | Repeat renders produce identical PCM in the tested executable/environment |
| Release | macOS toolchain, formatting, Clippy, all tests, offline installed CLI, documented listening status |

Finite/non-silent measurements and PCM repeatability are automated evidence.
They do not establish a human listening review or cross-platform bit identity.
