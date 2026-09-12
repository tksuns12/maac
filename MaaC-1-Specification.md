# MaaC/1
## Declarative music and production language — proposed specification, draft 1.0

**Status:** Original design proposal, 7 September 2026. This is not an existing standard, shipping product, or claim of compatibility with an existing renderer. MaaC is a working name. Normative language defines the proposed behavior; it does not imply that a complete implementation accompanies this document.

**Purpose:** Describe a finite musical performance and its production state as inspectable, typed, addressable source. A human, visual editor, or language model may author the same document. No model is required to interpret or execute it.

**Central rule:** Every render-affecting value must be an explicit value, a formally defined transformation, or a reference to an identified implementation or immutable asset. Subjective instructions are not executable language constructs.

**Design guidance:** The [project design principles](docs/design-principles.md)
guide future evolution of the musical core, libraries, engine extensions, and
tools without changing this specification's existing conformance requirements.

---

## 1. Scope and terminology

MaaC represents notes, timed triggers, expression, reusable patterns, arrangement, recorded audio, automation, modulation, instruments, processing, and routing. It does not require a piece to be electronic, tonal, repetitive, or in twelve-tone equal temperament.

`MUST` is a conformance requirement; `SHOULD` allows a documented exception; `MAY` is optional. The statements in sections 2–26 are normative unless marked otherwise. Examples are illustrative unless designated as conformance vectors.

The language is declarative data with finite composition operations. It has no arbitrary JavaScript, network access, unbounded loops, implicit humanization, natural-language effects, or model calls. An external program may generate a document; its generated document, not that program's intention, is what MaaC executes.

There are three distinct representations:

1. **Source graph:** Named definitions and instances, including reusable patterns and explicit overrides. This is the editable authority.
2. **Resolved performance:** A finite set of scheduled events, expression functions, audio transports, and a processing graph. This is a derived, queryable view with source addresses.
3. **Render artifacts:** Audio, analysis, or exported interchange files, accompanied by a manifest. These are derived from a source graph, dependencies, and an execution environment.

A source-preserving editor also maintains a concrete syntax tree for comments and formatting. It MUST NOT flatten an entire source document merely to display or edit one note.

### 1.1 Deliberate exclusions

The core does not standardize engraving, lyric typography, interactive clip-launching decisions, arbitrary synthesis algorithms, or every manufacturer's plug-in parameters. Recorded performances and any decisions made during live use must be captured into finite events or audio to become a closed MaaC performance.

These exclusions do not mean that a project cannot use complex synthesis, time stretching, or a plug-in. Those are represented through the typed processor interface in section 17, with explicitly identified dependencies. The core defines several fully specified reference processors in section 18.

Labels such as `chorus` may name a region. A label has no acoustic behavior. Fields such as `mood`, `energy`, `make_better`, and `genre` are not accepted by any core object.

### 1.2 Conformance profiles

- **Document:** Parse, preserve, validate, normalize, and edit all core document types. Unsupported required extensions must be reported.
- **Performance:** Additionally resolve finite pattern instances, pitches, timing, expression, and automation. No audio engine is required.
- **Core Audio:** Additionally execute every reference processor in section 18 and the core audio-asset/transport profile.
- **Locked Render:** Additionally verify the dependency and execution manifest. This is an environment-qualified reproducibility claim, not a promise that arbitrary platforms produce identical bytes.

An implementation states its profiles and its supported extension identifiers. A Document implementation MUST NOT advertise audio rendering merely because it can store an opaque processor state.

### 1.3 Implemented local sound-library extension

The normative [reusable instrument contract](docs/instruments.md) defines this repository's local-library extension: library documents, hash-pinned imports, instruments, presets, explicit WAV wavetables, and versioned `synth.* /1` voice/shared processors. It specifies the additional declaration and namespace rules, public control interfaces, synthesis behavior, and resource limits. These declarations opt into the extension; they do not change existing core processor behavior. The [capability matrix](docs/capabilities.md) identifies the implemented subset, and [performance-plan versions 1, 2 and 3](docs/performance-plan.md) define the separate rendering interchange format.

The normative [plucked-string implementation contract](docs/plucked-string.md)
adds the approved `synth.pluck/1` internally stateful string processor and the
separate `std/acoustic/1.0.0` guitar library. It fixes excitation, fractional
delay, live tuning, decay, strict plan fields, and memory/work accounting before
implementation. It preserves source grammar, core processor and plan versions,
public graph DAG rules, and frozen basic-library bytes. Numerical verification
and user listening acceptance remain separate from this design approval.

### 1.4 Native production extension

The normative [native mixing and delivery contract](docs/production.md) defines
the explicitly required capability `maac.production/1`: native `fx.eq/1`,
`fx.compressor/1`, and `fx.reverb/1` nodes, plus named master/stem deliveries,
resampling, encoding, and final-artifact loudness analysis. These processors
use existing `node` syntax and require no external executable implementation
asset. Named deliveries use an existing render-affecting `extension` object
with a hash-pinned, self-contained [schema](production.schema.json).

This capability preserves `maac 1` semantics and all Core Audio conformance
obligations; supporting it does not establish Core Audio conformance. The
grammar, generic syntax-tree schema, and implemented performance-plan versions
remain unchanged. An **experimental Rust implementation** and runnable
[example](examples/production.maac) are available. The bounded
[fixtures](production-conformance.json) remain specification evidence; they do
not establish rendering or listening conformance. Analyzer
`maac.analysis.bs1770-5/2` selects the approved single-stage four-times Annex 2
true-peak profile. See the applicable fixture evidence and historical 16×
comparison in the [metering report](docs/production-metering-evidence.md).

## 2. File format and lexical rules

The recommended source extension is `.maac`. Files are UTF-8. A file begins with:

```maac
maac 1;
```

Whitespace is insignificant outside strings. `//` starts a line comment. `/* ... */` is a non-nesting block comment. Semicolons terminate fields; object blocks do not require a following semicolon. Strings use JSON string escapes. An identifier matches `[A-Za-z_][A-Za-z0-9_]*` and is case-sensitive. Identifier length is at most 128 ASCII bytes. Unicode may appear in string labels, not identifiers.

Numbers are signed integers, finite decimals, or fractions with a strictly positive integer denominator. Decimal syntax requires digits on both sides of a decimal point: use `0.8`, not `.8`. Exponent notation, NaN, infinity, and negative zero in canonical output are forbidden. A fraction such as `1/3q` means `(1/3) × one quarter-note unit`; it does not mean division by a unit-bearing value.

Object syntax is uniform:

```maac
kind identifier {
  field = value;
  child_kind child_identifier { field = value; }
}
```

Values are numbers, quantities, strings, booleans, symbols, references, ordered lists, ordered tuples, unordered records, or one of the closed set of constructors defined in section 3. General expression evaluation is not part of v1.

```maac
length = 4q;
points = [(0q, 120bpm, step), (16q, 128bpm, step)];
params = { attack = 5ms; level = 0.2; };
target = &synth:events;
```

Top-level identifiers are unique across all kinds. Child identifiers are unique within their parent. A field and child of the same name within a parent are forbidden. Declaration order does not determine meaning. Duplicate fields, duplicate IDs, unknown core fields, and unknown core kinds are errors.

### 2.1 Conventional project entrypoint

A project SHOULD use `main.maac` as its master composition source, with one
`project`, visible final routing/gain, and its full arrangement. Existing
declaration order remains unrestricted and forward references remain valid;
existing explicit source filenames need not be renamed. Imports remain
library-only and do not merge compositions.

The normative [project-entrypoint contract](docs/project-entrypoint.md) defines
`check`, `compile`, and `build` entry selection: omitted input selects
`cwd/main.maac`; directory input selects that directory's `main.maac`; explicit
file input remains compatible. It also defines caller-selected default/song
execution allowances, separate from §1.2 conformance profiles, while preserving
all other bounds and the source/plan versions. The
[entrypoint delivery report](docs/project-entrypoint-delivery.md) records the
implemented contract's integrated checks and native source-free render evidence.

## 3. Values and types

### 3.1 Exact values

All source numbers are exact rationals. The canonical authored graph reduces
each rational to numerator/denominator form without changing authored field
presence or the spelling of a typed unit. A denominator of zero is a syntax or
type error. This applies to decimals too: `0.1` is exactly `1/10` before DSP
conversion.

| Type | Surface form | Semantic / execution rule |
|---|---|---|
| Dimensionless number | `0.8`, `3/2` | Reduced rational |
| Musical duration or position | `4q`, `1/3q` | Quarter-note units |
| Physical duration or position | `20ms`, `1.25s` | Seconds; `1ms = 1/1000s` |
| Sample-frame coordinate | `48000frame` | Integer frame count; asset or engine context required |
| Frequency | `440Hz`, `1.5kHz` | Hz; `1kHz = 1000Hz` |
| Tempo | `120bpm` | Quarter notes per minute |
| Pitch interval | `700ct` | Cents |
| Logarithmic amplitude | `-6dB` | Numeric dB, amplitude multiplier `10^(dB/20)` |
| Pitch | `C4`, `key(60)`, `degree(7, &tuning)`, `440Hz` | Section 7 |
| Reference | `&riff.n1`, `&synth.params.level`, `&synth:out` | Resolved typed address |

`q` never changes meaning with the meter. In 6/8, one bar is `3q`, not `6q`. The language has no context-dependent naked “beat” unit and no `bar` duration unit.

The authored graph preserves explicit unit tags such as `ms`, `s`, `q`, and
`Hz`, including when their rational values are equivalent. A normalized
execution view may convert those quantities to common units.

Clock positions and durations are distinct semantic types even where their literals look the same. Adding a position to a position is not an implicit operation. Units are not inferred from a field name. `at = 4;` is invalid where musical or physical time is required.

### 3.2 Constructors

The only core value constructors are:

- `key(k)`: integer twelve-tone key index, with key 69 at 440 Hz. Not restricted to a MIDI transport range.
- `degree(k, &tuning)`: integer index into an explicitly declared tuning.
- `ratio(r, f)`: positive dimensionless frequency ratio times a positive frequency literal.
- `bar(b, u)`: musical position from a one-based bar number and a one-based, possibly fractional, denominator-note position within that bar, using the project's meter map.

For `bar`, `b` is any integer, `u` is rational, and `1 <= u < numerator+1`. Bar 1 begins at `0q`; bar 0 is the preceding bar. The next bar is addressed as `bar(b+1, 1)`, not by an out-of-range beat. `bar(...)` is allowed only in global score-position fields, not pattern-local positions or durations. Semantic normalization lowers it to exact `q` in the separate execution view. Lowering it into the authored source is an explicit edit that adopts the current meter resolution; it is never silent, and a later meter edit does not re-resolve the resulting `q` address.

No other function name is executable core syntax. In particular, `random()`, `humanize()`, and `generate()` do not resolve inside a core document.

### 3.3 References

`&a.b` addresses a child of `a`; `&a.params.x` addresses a processor parameter. `&a:out` addresses a named port. References are absolute within the single file, not relative to the source line. A reference must resolve to the expected type.

A processor may have a port and parameter with the same local name because the colon and `.params.` addresses disambiguate them. IDs are identities, not display names. Renaming a display label does not rename the object. An explicit ID rename must update every reference atomically.

## 4. Core object inventory

There is exactly one `project`. Other top-level kinds are:

| Kind | Function |
|---|---|
| `tempo`, `meter`, `tuning` | Musical coordinate systems |
| `pattern` | Finite reusable event content |
| `track`, `place` | Event destinations and arrangement instances |
| `curve`, `automation`, `modulate` | Expression and parameter functions |
| `asset`, `audio` | Immutable external data and arranged audio transports |
| `node`, `connect` | Processors and explicitly typed routing |
| `region` | Named score interval, without playback behavior |
| `extension` | Namespaced data governed by a required external schema |

Nested pattern kinds are `note`, `hit`, `message`, and `use`. A `note` may contain `expression`. A `place` may contain `override` and `insert`. An `insert` contains exactly one `note`, `hit`, or `message`. These nesting locations are exclusive unless an extension schema explicitly adds another location.

All objects may carry the optional string field `label`. No other implicit fields exist. The field definitions below specify defaults. Defaults are expanded into the normalized source graph, not silently chosen by a host.

## 5. Project and global maps

```maac
project demo {
  score = [0q, 32q];
  rate = 48000Hz;
  tempo = &clock;
  meter = &metre;
  output = &master:out;
  tail = 2s;
  seed = 1;
  requires = [];
}

tempo clock {
  points = [(0q, 120bpm, step)];
}

meter metre {
  points = [(0q, 4, 4)];
}
```

`score` is a required ordered pair `[start, end]` of musical positions with `start < end`. Negative start times permit pickups and explicitly represented preroll. `rate` is a required positive integer Hz value. The reference audio profile must support 48000 Hz; additional rates are implementation capabilities, never silent substitutions.

`tempo` and `meter` are required references. `output` is an audio-output-port reference, optional only for Document/Performance-only projects. `tail` is a nonnegative physical duration, default `0s`. `seed` is an unsigned 64-bit integer, default 0. `requires` is a list of exact extension identifiers, default empty.

Rendering begins from reset processor state at `score.start`. There is no inferred prehistory. New score events may begin only in `[score.start, score.end)`. Held notes are released at score end; one-shot and effect tails may continue during `tail`. Automation freezes at its score-end value during the tail; sources whose transport ends before that remain silent. No new hit, message, or note-on is issued during the tail. At tail end output stops without an implicit fade or limiter.

Source event durations may extend past score end, but effective note-offs are truncated there. An effective note-on outside the score is an error, not an automatically relocated event. A partial render is an exact crop of the full performance from its reset origin, or an equivalent verified checkpoint render. Seeking MUST NOT reset an instrument at the crop boundary and pretend the result is equivalent.

### 5.1 Tempo map

`tempo.points` is a nonempty list `(q_position, positive_bpm, shape)`, strictly increasing in score position. Shape is `step` or `linear`; the last shape must be `step`. Shape describes the outgoing interval. Before the first and after the last point, the nearest endpoint tempo is held.

Let `B(q)` be the resulting positive quarter-notes-per-minute function. The
absolute physical-time origin is `T(0) = 0`. Absolute physical time is:

`T(q) = integral from 0 to q of 60/B(x) dx`.

For a project, define the reset origin `O = T(score.start)` and the project
end `E = T(score.end)`. At sample rate `rate`, engine frame `n` represents the
physical instant `O + n/rate`. The map and any absolute seconds anchors use
`T(0) = 0` independently of a project's reset origin `O`.

For a step segment of length `d`, elapsed seconds are `60*d/B`. For a linear-in-score-time segment starting at `a` with tempo `Ba` and slope `k` bpm per q:

`T(q)-T(a) = 60/k * ln((Ba + k*(q-a))/Ba)` when `k != 0`.

The constant-tempo formula applies when `k = 0`. “Linear tempo” never means linear in seconds. `T` is strictly increasing and invertible. All later score/seconds conversion uses this one map.

### 5.2 Meter map

`meter.points` is a nonempty list `(q_position, numerator, denominator)`. The first point is at `0q`; subsequent points are strictly increasing and must occur on bar boundaries under the preceding meter. The numerator is a positive integer. The denominator is a positive power of two, at most 1024. Meter before zero is the first meter extended backward.

A bar occupies `numerator * 4/denominator` q. Meter is a coordinate and display structure; it does not quantize events, alter tempo, or imply accents. Independent pattern lengths provide polymetric playback even though the core has one global bar-numbering map. Track-specific printed meter is an engraving extension, not another conflicting physical clock.

## 6. Time, offsets, and scheduling

Score event intervals are half-open `[on, off)`. Positive note duration is mandatory. Simultaneous notes need not have distinct pitches. Their identities remain distinct even when they share a receiver and key.

A note has independent physical `onset_offset` and `release_offset`, both default `0s`. After all existing pattern transforms, pattern/use/placement cuts, and occurrence edits, let `q_on` and `q_off` be the final musical coordinates before project-end clamping. Inherited cuts occur before occurrence edits; an occurrence override's final duration is not recut.

`on_seconds = T(q_on) + onset_offset`

`raw_off_seconds = T(q_off) + release_offset`

`off_seconds = min(raw_off_seconds, E)`.

These effective-time checks are additional to source-coordinate and placement validation. A valid event must satisfy `O <= on_seconds < E` and `off_seconds > on_seconds` before the existing frame-ceiling and subsample validation; otherwise it is `E_INTERVAL`. Project-end truncation happens in physical time after applying `release_offset`: preserve `q_off` until this calculation, and do not first clamp it to `score.end`. To delay an entire note by 20 ms without changing its physical gate length, set both offsets to `20ms`. Changing only onset offset changes the gate length. A hit or message has only onset offset.

For example, at 120 bpm a score ending at `1q` has `E = 0.5s`. A valid event whose raw `q_off` is `2q` and whose `release_offset` is `-250ms` has `raw_off_seconds = 0.75s` and therefore `off_seconds = 0.5s`. If a pattern/use/placement cut changes `q_off` to `1q`, the cut is applied in score time before the release offset, so `off_seconds = 0.25s`.

Physical offsets are not multiplied by a pattern's musical stretch. They are not fractions of a beat. Moving a note across a tempo change recomputes `T` before offsets are applied.

### 6.1 Sample scheduling

For event physical time `t`, the engine frame relative to the reset origin is:

`frame(t) = ceil(rate * (t - O))`.

An event is never scheduled earlier than its specified physical time. The quantization error is less than one sample period. Core events are processed at the start of their scheduled frame, before that frame's output sample is calculated. A positive physical gate that collapses to zero frames is `E_SUBSAMPLE_NOTE`; a renderer must not silently lengthen it.

At one frame, order is: parameter evaluation, `message` events, note-offs, then note-ons and hits. Equal-class events sort by explicit `order`, then stable event address in unsigned UTF-8 byte order. `order` defaults to zero. Initial per-note expression is part of note-on state, not a later unaddressed controller event. This deliberately specifies what happens when distinct sub-sample times quantize to the same frame.

Frame calculation is mathematically defined, not “whatever double precision produces.” Step-tempo rational cases must use exact arithmetic. Other conversions must establish the correct ceiling, increasing precision where needed; inability to establish a boundary is `E_TIME_PRECISION`, not permission to guess. Floating-point DSP output is a separate concern.

Continuous automation is evaluated at the physical sample instant and converted back with `T^-1` when it uses a score clock. Its step transitions therefore take effect at the first sample at or after their specified time, matching the scheduling rule.

## 7. Pitch and tuning

`C4` is key 60. `A4` is key 69. The formula for an integer key `k` is:

`frequency(k) = 440 * 2^((k-69)/12) Hz`.

Letter syntax is `[A-G]`, zero to two identical `#` or `b` accidentals, and a signed octave number. Its key is `12*(octave+1) + natural_semitone + accidental_offset`. B-sharp and C-flat cross octave boundaries according to this arithmetic. Key signatures never silently alter letter pitches.

`key(k)` has the same acoustic meaning, without spelling. A positive Hz quantity or `ratio(r, f)` gives absolute frequency directly. The language does not impose the MIDI 0–127 range; a particular receiver may impose declared pitch bounds. Nonpositive frequency is invalid. The reference sine processor rejects frequencies at or above the engine Nyquist frequency.

```maac
tuning nineteen {
  period = 1200ct;
  steps = [0ct, 1200/19ct, 2400/19ct, 3600/19ct,
           4800/19ct, 6000/19ct, 7200/19ct, 8400/19ct,
           9600/19ct, 10800/19ct, 12000/19ct, 13200/19ct,
           14400/19ct, 15600/19ct, 16800/19ct, 18000/19ct,
           19200/19ct, 20400/19ct, 21600/19ct];
  reference_index = 0;
  reference_frequency = 440Hz;
}
```

`period` is positive cents. `steps` is nonempty, strictly increasing, begins at `0ct`, and remains below period. With `N` steps, use floor division for negative indices: `k = N*m + r`, `0 <= r < N`. Define `c(k) = m*period + steps[r]`. Then `degree(k, tuning)` is `reference_frequency * 2^((c(k)-c(reference_index))/1200)`.

A transposition in cents multiplies frequency by `2^(cents/1200)`; it does not round to a tuning degree. Source spelling may be retained separately as inert notation data. Arithmetic must not assume that all music is a sequence of MIDI integers.

## 8. Notes, hits, and messages

```maac
pattern bass_riff {
  length = 4q;
  note n1 { at = 0q; dur = 1/2q; pitch = C2; velocity = 0.8; }
  note n2 { at = 3/2q; dur = 1/2q; pitch = G2; velocity = 0.7; }
  note n3 { at = 3q; dur = 1q; pitch = Bb2; velocity = 0.75; }
}
```

A `note` requires `at`, `dur`, and `pitch`. `at` is nonnegative pattern-local q; `dur` is positive q. `velocity` is dimensionless `[0,1]`, default 1. `release_velocity` is `[0,1]`, default 0.5. `onset_offset` and `release_offset` are seconds, default zero. `order` is an integer, default zero. Notes are not shortened merely because another note of the same pitch starts.

A `hit` requires `at` and a string `key`, identifying a declared unpitched trigger on its destination, such as a sample-kit key. It has `velocity`, `onset_offset`, and `order` with the same defaults. It has no gate duration or implicit note-off; the receiver's declared trigger behavior determines its tail.

A `message` requires `at`, an exact protocol identifier string, and `bytes`, a list of integers from 0 through 255. It has `onset_offset` and `order`. Messages are transport payloads, not host actions. The receiving adapter must explicitly advertise the protocol and define its effect. There is no core network or external-hardware message receiver. A closed render must reject uncaptured external-device dependencies.

Overlapping raw protocol note messages cannot automatically be reconstructed into native note identities. Importers must state that limitation rather than claiming a lossless semantic conversion.

### 8.1 Per-note expression

A note may contain:

```maac
expression bend {
  kind = pitch;
  curve = &bend_shape;
}
```

Kinds are `pitch` (cents offset, default 0ct), `gain` (nonnegative amplitude multiplier, default 1), `pressure` and `timbre` (dimensionless `[0,1]`, default 0). At most one expression of each kind exists per note. The receiver must advertise support; unsupported expression is an error in faithful render mode.

The referenced curve is evaluated on the note's effective gate. A normalized clock maps 0 to actual scheduled note-on and 1 to actual scheduled note-off. A seconds clock measures physical time from scheduled onset. A score clock maps the effective gate affinely onto `[0, final_note_duration_q]`; this explicitly distributes onset/release offsets across the expression timeline rather than introducing an undeclared extra clock. A note's attached score-clock curve coordinates are multiplied by the product of its inherited musical stretch factors; normalized and seconds coordinates are not. This produces an instance-specific curve view without modifying a shared curve definition. A duration-only occurrence override changes the gate but does not additionally rescale those score-clock curve coordinates. Outside the curve's points, endpoint values hold. During release tails, expression holds its gate-end value.

## 9. Finite patterns and composition

A `pattern` has a required positive musical `length`. Its children are leaf events and/or `use` objects. Every child onset must satisfy `0 <= at < length`. A note may end after length; length defines the repeat interval, not an automatic note-off. Pattern definitions form a directed acyclic reference graph.

```maac
pattern eight_quarters {
  length = 8q;
  use first {
    pattern = &bass_riff;
    at = 0q;
    count = 2;
    stretch = 1;
    transpose = 0ct;
    boundary = spill;
  }
}
```

A `use` requires `pattern` and nonnegative musical `at`. `count` is a positive integer, default 1; `stretch` is positive dimensionless rational, default 1; `transpose` is cents, default 0ct; `boundary` is `spill` or `cut`, default `spill`.

The occupied repetition span is `count * stretch * source.length`; it must fit in the parent pattern's length. For repetition `i`, a child at `a` with gate `d` maps to:

`at_parent = use.at + stretch*(i*source.length + a)`

`dur_parent = stretch*d`.

Pitch transpositions add through nesting. Physical offsets do not stretch. A `cut` boundary truncates note gate ends to that repetition's score boundary before physical offsets are applied. `spill` preserves the entire gate, even across later repetitions. Cut never retriggers a note or truncates an instrument's subsequent release envelope.

Simultaneous `use` objects implement layering. Sequentially positioned uses implement concatenation. Independent lengths and repetition counts implement finite polymeters. Silence is simply absence of events; an empty pattern with a positive length is valid.

There is no recursive pattern call, mutable counter, implicit global cycle, or infinite generator. A host may offer a live looping mode, but that transport behavior is outside a closed document. Probability, Euclidean rhythms, reversal, quantization, and swing may be authoring operations; their materialized events or explicit finite uses are stored. They are not hidden runtime interpretation.

This boundary is intentional: the core preserves compact reusable structure without requiring a language runtime to discover the resulting music. A future pure-generator profile may be added, but is not claimed by this version.

## 10. Tracks, placement, and event identity

A `track` has an optional event `target` port. A track used by a note/hit/message placement requires that target. Tracks may group audio objects without an event target. Tracks do not create mixers, faders, pan laws, or implicit connections.

```maac
track bass { target = &bass_synth:events; }

place bass_main {
  pattern = &bass_riff;
  track = &bass;
  at = 0q;
  count = 8;
  stretch = 1;
  transpose = 0ct;
  boundary = spill;
}
```

A `place` has the same repetition and transformation fields as `use`, plus required `track`. Its `at` is a global score position. Its occupied repetition span must fit within the project score. Each expanded leaf carries a structured event address:

`(placement_id, repetition_index, [use_id, repetition_index ...], leaf_id)`.

For example, `bass_main/7/n2` denotes repetition 7, zero-based, of leaf n2 in that placement. A nested event could be `bass_main/0/first/1/n2`. These are structural tuples, not parsed display names; their display form escapes nothing because IDs are restricted ASCII and do not contain `/`.

Adding an unrelated note does not renumber existing event addresses. Changing count preserves the addresses of surviving repetitions. Moving a note, changing its pitch, or changing its label does not change its identity. Moving a leaf to a different containing pattern is a structural change and must update or reject dependent instance edits.

An expansion query returns source addresses and the complete event interval, even when the query intersects only its middle. It must not shorten a sounding note just because the user queried a narrow window.

## 11. Instance overrides and inserts

An `override` belongs to one `place`:

```maac
override late_note {
  event = "7/n2";
  set = { onset_offset = 20ms; release_offset = 20ms; };
}
```

`event` is a relative expanded event address within that placement. `set` is a record of replacements for leaf fields; `delete` is an alternative boolean field that must be `true`. Exactly one of `set` and `delete` is present. There is at most one override per addressed event. The target must exist before overrides; dangling overrides are errors.

Overrides apply **after** all pattern expansion, musical stretch, and transposition. `set.at`, when used, is a musical offset from the placement's global origin, not from the source pattern or the selected repetition. `set.dur` is the final musical duration. `set.pitch` replaces the final pitch without reapplying placement transposition. An override cannot change identity, event kind, destination track, or introduce another source reference.

This final-state rule makes local patches inspectable: replacing the pitch with `C3` means C3, not “C3 and then whatever upstream transposition happens to do.” Allowed replacement fields are the fields of the leaf's kind, except label and order may also be changed. Expression children are not replaced through `set`; they are edited by child-addressed patch operations on a materialized instance variant (see below).

An `insert` adds one new leaf to a placement, in placement-local final q coordinates. Its child may include note expression. It is not repeated automatically. The inserted event address is `insert_id/leaf_id`, in a disjoint address form whose first component is an identifier rather than a repetition integer.

For complex per-occurrence expression changes, `materialize-instance` creates an explicit new pattern for the selected placement, preserving a source-address mapping, then redirects that placement. This is an explicit authoring operation, never an automatic consequence of moving a note in a piano roll. Sharing is broken only by a declared edit.

## 12. Curves and parameter automation

```maac
curve opening {
  clock = score;
  points = [(0q, 500Hz, exponential), (16q, 2000Hz, step)];
}

automation filter_open {
  target = &filter.params.cutoff;
  curve = &opening;
  at = 0q;
}
```

A `curve` requires `clock` and a nonempty ordered list of `(position, value, outgoing_shape)` points. Clock is `score`, `seconds`, or `normalized`. Point positions are curve-local, using q, seconds, or dimensionless rationals respectively; the consuming feature defines their time origin. The first position must be zero, and positions strictly increase. A normalized curve's final position must be 1; values must match the target's dimension. The last point's shape must be `step`.

Shapes are `step`, `linear`, and `exponential`. At a knot the new point's value applies; the function is right-continuous. With `u = (x-x0)/(x1-x0)`:

- step: `v(x) = v0` before x1;
- linear: `v(x) = v0 + (v1-v0)*u`;
- exponential: `v(x) = v0*(v1/v0)^u`, requiring both endpoint numbers to be strictly positive and the target to permit ratio interpolation.

Interpolation takes place in the explicitly declared value unit. Linear interpolation from -12dB to 0dB is linear in decibels, not amplitude. Exponential interpolation is forbidden for logarithmic dB and cents targets and for enumerations. Enumerations, booleans, and discrete parameters permit step only. There is no inferred smoothing; a processor that smooths parameters must declare its exact smoothing behavior.

An `automation` targets a node parameter and references a score- or seconds-clock curve. Normalized curves are not global automation. `at` is required: an absolute q coordinate (`at_q`) for a score-clock curve; either an absolute q coordinate (`at_q`) or absolute physical seconds (`at_s`) under `T(0)=0` for a seconds-clock curve. Omitting required `at` is `E_RANGE`; an anchor unit incompatible with the curve clock is `E_UNIT`. For global automation, curve-local positions are relative to the physical anchor instant: `T(at_q)` for a q anchor or `at_s` for a seconds anchor. A q anchor for a seconds curve converts once through `T`, after which the curve runs in physical seconds. At physical time `t`, global automation uses `t* = min(t, E)` during the tail and computes its relative curve position as follows:

- score clock: `x = T^-1(t*) - at_q`;
- seconds clock with a q anchor: `x = t* - T(at_q)`;
- seconds clock with a seconds anchor: `x = t* - at_s`.

For `x < 0`, the node's base parameter applies. At `x = 0`, point zero applies by right continuity; after the last point, the final point value holds. The `t*` clamp is only for global automation: audio transports and seconds-clock LFOs continue on physical time and retain their existing contracts. There is no additional anchor bound; an anchor may precede the reset origin. If the physical anchor instant—`T(at_q)` for a q anchor or `at_s` for a seconds anchor—is after `E`, global automation remains at its base value throughout the tail; if that physical anchor instant equals `E`, point zero applies there by right continuity.

There is at most one global replacement automation per parameter. No “last writer wins,” track-order priority, or overlapping-lane blend is implied. Multiple desired segments are represented as one curve. Missing endpoints and undeclared crossfades must not be invented by a host.

## 13. Modulation

```maac
node wobble {
  type = "core.lfo/1";
  config = { period = 2q; wave = sine; phase = 0; };
}

modulate vibrato {
  target = &filter.params.cutoff;
  from = &wobble:out;
  amount = 100Hz;
}
```

A `modulate` requires a continuous numeric parameter target, a dimensionless scalar control-output port, and `amount` in the parameter's native numeric unit. It contributes an additive offset, not a replacement. Let `b(t)` be the base or automated value and each control signal be `m_i(t)`:

`raw_parameter(t) = b(t) + sum(amount_i * m_i(t))`.

Contributions are summed in modulation-ID order for numerical reproducibility. Parameter range policy is part of the processor descriptor: `error` fails on an out-of-range value, and `clamp` clips to the declared range. There is no silent host-selected clipping policy. Integer or enum targets cannot receive this form of modulation.

Multiplicative behaviors can be expressed with an amplitude parameter and an appropriately constructed control signal; v1 does not provide a second implicit parameter-combination algebra. Modulation signals are scheduled as graph dependencies, so a control loop must satisfy the same causality rules as audio routing.

## 14. Assets and arranged audio

### 14.1 Immutable assets

```maac
asset take {
  kind = audio;
  path = "assets/take.f32";
  hash = "sha256:<64 lowercase hexadecimal digits>";
  format = "pcm_f32le_interleaved/1";
  rate = 48000Hz;
  channels = 1;
  frames = 192000;
}
```

The hash text above is an explanatory placeholder, not a valid literal digest. A usable asset declaration must contain the actual digest of the complete asset bytes.

An `asset` requires `kind`, `path`, and `hash`. Kind is `audio`, `blob`, `descriptor`, or `module`. Path is a package-relative retrieval hint; hash is identity. Audio also requires `format`, positive integer-Hz `rate`, positive integer `channels`, and nonnegative integer `frames`. Other kinds do not carry audio fields.

The core audio format is interleaved IEEE-754 binary32 samples, little-endian, with no header. File byte length must equal `frames * channels * 4`. Samples must be finite; NaN and infinity are invalid. Subnormal and signed-zero samples are valid source data. Values outside [-1,1] are not automatically clipped. The same exact bytes and metadata always describe the same source signal.

This deliberately simple normative format avoids making a codec library part of the language. WAV, AIFF, FLAC, compressed audio, and container decoding are import/extension capabilities. A faithful importer may convert them to the core format and retain the original file as a blob. A decoder extension must pin its decoder behavior in the render manifest.

Resolution is offline and package-relative by default. Absolute paths, traversal outside the package, symbolic-link escapes, and implicit network fetches are rejected. Asset replacement is an explicit edit updating both content identity and affected metadata. Missing assets never become silent empty audio in faithful render mode.

### 14.2 Audio objects

An `audio` object is both an arranged transport and an audio-output source, with a port named `out`. It may have an optional `track` reference for grouping. Its signal is connected explicitly, just like a node's output.

```maac
audio vocal_clip {
  asset = &take;
  at = 8q;
  source = [0frame, 192000frame];
  mode = rate;
  speed = 1;
  reverse = false;
  gain = 1;
  fade_in = 5ms;
  fade_out = 20ms;
  fade_shape = linear;
}
```

Required fields are `asset`, `at`, `source`, and `mode`. `at` may be a global q coordinate (`at_q`) or absolute physical seconds (`at_s`) under `T(0)=0`; seconds are not render-relative. Define the transport start `S` as `T(at_q)` or `at_s`, respectively. Both forms require `O <= S < E`. At engine frame `n`, physical time is `t_n = O + n/rate` and elapsed transport time is `tau = O + n/rate - S`; use this elapsed time for transport position, including its fractional playback phase at the first scheduled frame, rather than resetting `tau` to zero. Existing tail behavior is retained. The source is a half-open pair of integer asset frames `[a,b]` with `0 <= a < b <= asset.frames`. `gain` is a nonnegative amplitude multiplier, default 1. Fades are nonnegative seconds, default 0s; `fade_shape` is `linear` or `equal_power`, default linear. The signal is zero outside the transport interval.

For example, at 120 bpm a score `[4q, 8q]` has `O = 2s` and `E = 4s`: `at = 2.5s` is valid and begins at frame `24000` at 48000 Hz, while `at = 0.5s` is invalid because it precedes `O`. With score `[-1q, 1q]`, the reset origin is `O = -0.5s`, so `at = 0s` is a valid absolute physical start.

In `rate` mode, `speed` is positive dimensionless rational, default 1, and `reverse` is boolean, default false. Let `N=b-a`. Define a sliced discrete buffer `z[j]=asset[a+j]`, or `asset[b-1-j]` for reverse, for `0 <= j < N`. Its values outside that range are zero. At elapsed physical time `tau` from the clip start, the source coordinate is `u = tau * asset.rate * speed`, and output is the linear interpolation of z at u. Duration is `N/(asset.rate*speed)` seconds. Rate changes intentionally change both duration and pitch.

Core interpolation uses `j=floor(u)`, `f=u-j`, and `(1-f)*z[j]+f*z[j+1]`. It is a fully specified reference resampler, not a claim of high-end anti-aliasing quality. A different resampler must be an explicitly identified audio-transport extension.

For elapsed clip time `tau` and duration `D`, a linear fade-in is `clamp(tau/fade_in,0,1)` and fade-out is `clamp((D-tau)/fade_out,0,1)`. A zero fade length means multiplier 1. Equal-power shapes replace each nonzero fade factor x by `sin(pi*x/2)`. The two envelopes multiply, including when their intervals overlap. They multiply `gain` and the reconstructed signal. Crossfades are expressed by overlapping clips and explicit fades; no automatic fade is added to an edit.

An audio transport may extend beyond score end. It continues through the declared tail, because it has already started, but output is always truncated at render end. A crop that would require earlier playback must extend the project's reset origin explicitly.

### 14.3 Warp modes

`warp_rate` and `warp_preserve` require musical `at` and an ordered `warp` list of `(local_q, absolute_source_frame)` pairs. The first local q is zero and the last is positive; both local times and source frames strictly increase. The first/last source frames equal the declared source interval endpoints. Fractional source frame anchors are not accepted in v1.

```maac
warp = [(0q, 0frame), (4q, 96000frame), (8q, 192000frame)];
```

The last local q determines transport duration through the project's tempo map. At each output sample, the renderer computes local score time and linearly interpolates the corresponding source coordinate between anchors.

In `warp_rate`, that source coordinate feeds the core interpolator. Pitch changes with instantaneous playback rate, including changes caused by the tempo map. `speed` and `reverse` are forbidden in warp modes.

In `warp_preserve`, an additional `processor` reference names a module asset whose required descriptor defines the stretch algorithm, settings, latency, initialization, and output contract. The source mapping is still explicit, but exact waveform output depends on that pinned module. The module must return the declared source channel count and the requested output transport duration. It must handle any internal lookahead without changing the declared transport start. This is an extension capability, not a promise that all time-stretch algorithms sound alike. Unsupported preserve-pitch warping is an error, never an automatic fallback to rate warping.

## 15. Nodes, ports, and connections

```maac
node bass_synth {
  type = "core.sine/1";
  config = { voices = 32; };
  params = { attack = 5ms; release = 80ms; level = 0.2; };
}

node master {
  type = "core.sum/1";
  config = { channels = 2; };
}

connect bass_to_pan { from = &bass_synth:out; to = &bass_pan:in; }
connect pan_to_master { from = &bass_pan:out; to = &master:in; }
```

A core `node` requires `type`, an exact versioned processor identifier string. The local sound-library extension alternatively permits an `instrument` reference and optional matching `preset`; `type` and `instrument` are mutually exclusive. `config` and `params` are records, both default empty. Config fields affect structure or initialization and cannot be automated. Parameter fields may be automated according to the descriptor. External nodes also require `implementation`, a module-asset reference, and may carry `state`, a blob-asset reference. Core nodes forbid `implementation` and `state` unless their own definition explicitly allows one; their reset state is specified below.

`connect` requires `from` and `to` port references. Source and destination port kinds, dimensions, and channel counts must match exactly. Port kinds are audio with fixed channel count, scalar dimensionless control, or events with a declared event/protocol capability set. There is no implicit MIDI-to-audio conversion, channel duplication, mono/stereo conversion, gain change, or sample-rate conversion between engine nodes. Audio assets may have their own rate because their transport explicitly resamples them.

A single-input port accepts one connection. A summing input accepts multiple connections and declares the addition rule. A disconnected audio/control input is zero only if its descriptor marks it `zero_default`; otherwise it is an error. Event inputs merge incoming events under the section 6 schedule. Track event targets act as additional event-source connections for this purpose.

An output may feed several inputs. This fan-out does not copy a node's state. Two node instances referring to the same implementation always have independent state unless that implementation explicitly declares a prohibited shared-state dependency; such a dependency prevents closed-render conformance.

Track gain, inserts, sends, sidechains, parallel compression, and buses are all ordinary processors and edges. A send is not a special undocumented path. A graph editor may display convenient mixer strips, but the stored graph states exactly where the signal is taken.

### 15.1 Latency is explicit

Every processor descriptor reports its fixed technical latency in frames. A latency change invalidates a locked manifest. MaaC/1 does not add hidden delay compensation. An authoring operation may calculate alignment delays and insert named delay nodes; those become source state. Algorithmic delays intended as sound and alignment delays both remain inspectable, even if UI labels distinguish them.

A graph validator should warn when parallel paths with different declared technical latencies recombine, but must not infer that every delay is a mistake. Exact transport alignment is a graph property, not an undocumented DAW preference.

## 16. Causality and graph execution

Each processor descriptor declares which current-sample inputs each output depends on. These feedthrough dependencies, combined with connections and parameter-modulation dependencies, form the **same-sample dependency graph**. That graph must be acyclic.

Explicit `core.delay/1` with at least one frame removes a same-sample dependence between its input and output. An opaque plug-in's positive reported latency does not automatically prove that it can break a feedback cycle. Only a processor explicitly declaring the required causal behavior may do so.

For each engine sample:

1. Read outputs of state delays from their stored history.
2. Evaluate zero-delay dependencies in a stable topological order, including required control signals, parameter values, events, and audio.
3. Commit state updates and delay-buffer writes for that sample.

Among equally available nodes, order by node ID in unsigned UTF-8 byte order. A sum with multiple inputs adds them in connection-ID order. A cyclic same-sample graph is `E_ALGEBRAIC_LOOP`.

Feedthrough declarations must account for parameter modulation as well as audio input. For example, a filter cannot modulate its own same-sample cutoff from its same-sample output without an explicit causal break. Feedback is permitted when this rule is satisfied; it is not silently stabilized, limited, or altered.

The mathematical reference semantics are sample-based. Engines may optimize in blocks only when equivalent for the claimed fidelity profile. An external processor may declare block-dependent behavior; its block size and scheduling adapter must then be locked, and its outputs cannot claim core sample-equivalent behavior without testing.

## 17. External processor descriptor contract

An external module is identified by a hashed `module` asset. Its executable format is an exact required capability identifier. A hashed JSON `descriptor` asset is named in its render-lock entry. The descriptor contains these required fields:

| Field | Contract |
|---|---|
| `id`, `version`, `abi` | Exact identity and supported host calling contract |
| `ports` | Unique names, direction, kind, channel count, cardinality, zero-default policy |
| `events` | Supported native note/expression kinds, hit keys, and message protocols |
| `config` | Field types, defaults, ranges, asset-reference types |
| `parameters` | Stable parameter IDs, source names, units, defaults, ranges, range policy |
| `parameter_rates` | Per-parameter `sample`, `event`, `block`, or `static`; interpolation support |
| `feedthrough` | Current-sample dependencies, including parameter dependencies |
| `latency_frames` | Fixed nonnegative technical latency for this configuration |
| `state` | Serialization version, reset semantics, save/restore capability |
| `determinism` | `declared_deterministic` or `nondeterministic`, plus documented dependencies |
| `permissions` | Files/assets, network, device access, process execution, shared memory |

A descriptor is not executable by itself. The ABI capability must have its own published contract. A host must not guess an ABI from a filename or start a native binary merely because a document refers to it.

On load, the host verifies hashes, instantiates the exact implementation, restores its complete state if supplied, applies source config according to the ABI initialization contract, then applies explicit source parameter values as deliberate overrides. The adapter must detect incompatible state/config combinations. A plug-in adapter cannot assume that exposed parameters reconstruct non-parameter state.

The graph stores stable parameter IDs and physical/native values. A display name such as “Cutoff” is not an identity. A normalized knob position is permitted only if the descriptor says the parameter's native unit really is normalized and pins the mapping. Automatable fields that a processor accepts only at block rate are an explicit fidelity restriction; a faithful render must use a declared sample-accurate adapter or fail rather than silently coarsen a sample-rate lane.

Unknown processor types may be preserved by a Document implementation, but must remain visibly unavailable. Substituting an oscillator for a missing synthesizer is not a conforming faithful render. A deliberate approximation mode must emit a loss report and cannot claim source-equivalent sound.

## 18. Reference processor library

The following processors belong to Core Audio. All accept finite binary64 internal values, initialize to the specified reset state, and emit finite samples or fail. They have zero reported technical latency except `core.delay/1`. Real-valued mathematical operations define their reference behavior; cross-machine byte identity additionally requires a locked numeric implementation.

All config and parameter keys not named here are invalid. All parameter defaults are explicit below. None of the processors automatically normalizes, limits, or dithers its output.

### 18.1 `core.sum/1`

Config: `channels`, required positive integer. Ports: summing audio `in`, audio `out`, both that channel count. No parameters. Output is the componentwise sum in connection-ID order. Empty input produces zero. Feedthrough: in to out.

### 18.2 `core.gain/1` and `core.fader/1`

Config: `channels`, required positive integer. Ports: single audio in/out. Gain has parameter `gain`, dimensionless nonnegative, default 1, sample-rate, range policy error. `y=gain*x`. Fader has parameter `level`, finite dB, default 0dB, sample-rate; `y=10^(level/20)*x`. Finite output is required. Neither has implicit smoothing. Feedthrough: in and parameter to out.

### 18.3 `core.pan/1`

No config. Mono single `in`, stereo `out`. Parameter `pan` in [-1,1], default 0, sample-rate, range policy clamp. Set `theta=(pan+1)*pi/4`; `left=x*cos(theta)`, `right=x*sin(theta)`. Center is an equal-power mono-to-stereo pan, not a stereo-balance control. Feedthrough: in and pan to out.

### 18.4 `core.matrix/1`

Config: positive integers `inputs` and `outputs`, and `coefficients`, an outputs-by-inputs rectangular matrix of finite dimensionless rational values. Ports: audio in with inputs channels, out with outputs channels. No parameters. For each output row, sum input channels multiplied by coefficients in ascending input-channel order. This is the explicit channel mapping/downmix/duplication primitive.

### 18.5 `core.delay/1`

Config: positive integers `channels` and `frames`. Single audio in/out of that channel count. No parameters. Output `y[n]=x[n-frames]`; values before reset are zero. Reported latency equals frames. No same-sample input-to-output dependency. Read history before writing the current sample.

### 18.6 `core.onepole/1`

Config: required positive integer `channels`. Single audio in/out. Parameter `cutoff`, positive Hz strictly below rate/2, default 1000Hz, sample-rate, range policy error. For each channel, `a=exp(-2*pi*cutoff/rate)` and `y[n]=(1-a)*x[n]+a*y[n-1]`. Previous y resets to zero. Feedthrough: in and cutoff to out. This identifies one particular filter algorithm, not an unspecified generic “low-pass sound.”

### 18.7 `core.sine/1`

Config: `voices`, positive integer default 64. Input `events` accepts native notes with pitch and gain expression; output `out` is mono audio. Unsupported pressure, timbre, hits, and messages are rejected. Voice overflow is an error; there is no hidden voice stealing.

Parameters: `attack` and `release`, nonnegative seconds, defaults 5ms and 80ms, event-rate; `level`, nonnegative dimensionless, default 0.2, sample-rate. All use error range policy. Attack is sampled at note-on; release at note-off. Pitch at or above rate/2, including expression, is rejected rather than aliased or folded.

A voice starts at scheduled frame N with phase zero and attack amplitude `min((n-N)/(attack*rate),1)`; attack=0 gives amplitude 1 immediately. At scheduled note-off M, release starts from the attack/sustain envelope value at M. For captured release R>0, amplitude thereafter is `A_M * max(1-(n-M)/(R*rate),0)`; R=0 ends immediately. A voice remains allocated until its envelope becomes zero after release. A note-on with velocity zero is valid but still consumes a voice under the same lifecycle.

At frame n, output is `velocity * gain_expression[n] * envelope[n] * level[n] * sin(phase[n])`. Then update phase by `2*pi*frequency[n]/rate`, modulo 2*pi. Pitch expression is a cents offset on the note's resolved base frequency. Voices sum in event-address order. Release velocity has no effect in this explicitly defined instrument. This is a device behavior, not a reinterpretation of the note data.

### 18.8 `core.kit/1`

Config: required positive integer `channels`; `voices`, positive integer default 64; and nonempty `samples`, a list of records `{ key = "name"; asset = &audio_asset; }`. Keys are unique; assets must have exactly the configured channel count. Input events accepts matching native hits only; audio out has that channel count. Parameter `level` is nonnegative dimensionless, default 1, sample-rate.

Each hit plays the entire asset once at its original physical rate, using the rate-mode core interpolator at speed 1. Voice amplitude is hit velocity times current level. No fade, choke group, looping, or tail truncation is inferred. Voices sum in event-address order and finish at source end. Voice overflow and unknown hit keys are errors. Chokes and more elaborate sample mapping require a separately specified processor, not undocumented magic keys.

### 18.9 `core.lfo/1`

Config: `period`, required positive q or seconds; `wave`, default sine, one of sine/triangle/saw/square; `phase`, default zero, dimensionless cycles. No inputs or parameters; scalar dimensionless control out.

For seconds period P, cycle coordinate is `(t-T(score.start))/P + phase`. For score period P, it is `(T^-1(t)-score.start)/P + phase`. Let f be its fractional part in [0,1). Sine is `sin(2*pi*f)`, saw `2*f-1`, square `1` for f<1/2 and `-1` otherwise, and triangle `1-4*abs(f-1/2)`. During the render tail, a seconds LFO continues and a score LFO holds at score end. This mirrors the absence of advancing musical score time during tails.

### 18.10 `core.constant/1`

No config or inputs. Parameter `value`, any finite dimensionless value, default zero, sample-rate. Scalar control out equals value. This parameter may be automated but not modulated from its own undelayed output.

### 18.11 `core.noise/1`

Config: required positive integer `channels`; optional unsigned 64-bit `seed`, default project seed. No inputs or parameters. Audio out emits independent counter-derived reference noise. For zero-based engine frame n and channel c, hash the concatenation of:

- ASCII bytes `maac-noise-1` followed by one NUL byte;
- seed as unsigned 64-bit little-endian;
- the node's full ASCII ID path followed by one NUL byte;
- n as unsigned 64-bit little-endian;
- c as unsigned 32-bit little-endian.

Take the first eight SHA-256 digest bytes as an unsigned little-endian integer, shift right by 11 to obtain r, and emit `2*(r/2^53)-1`. Adding a different track does not consume a shared random stream or change this node's noise. This expensive but simple reference primitive prioritizes specified behavior; an optimized implementation must produce equivalent values. It is not a license for a different random generator under the same type identifier.

## 19. Regions and non-rendering information

```maac
region chorus { span = [16q, 32q]; label = "Chorus"; }
```

A `region` requires a musical `[start,end]` span with start<end, within the project score. Regions may overlap or nest but do not inherit parameters, alter dynamics, or define automatic transitions. They give tools an exact answer to “which interval is the chorus?” rather than adding a subjective execution layer.

Display names, source comments, editor zoom, track color, and analysis annotations are not sound. UI state belongs in a sidecar such as `ui.json`, keyed by source IDs. Engraving, slurs, chord symbols, and articulation marks may be preserved as a declared notation extension; they cannot change playback without being lowered to explicit performance data or referring to a specified interpreter.

A chord in core is simultaneous notes. A rest is absent events over a duration. Sustain is represented by actual gate behavior or an explicit receiver-specific controller, not an unexplained “pedal” word. A slur in an engraving extension does not secretly shorten or lengthen notes.

## 20. Canonical data and hashes

### 20.1 Syntax interchange representation

The bundle defines a JSON schema for the **typed syntax tree**, not a full semantic validator. Its root contains `version` and an `objects` dictionary keyed by object ID. Every object has `kind`, `fields`, and `children`. Children are dictionaries keyed by local child ID. A source object's identity is its dictionary key, not its position in an array.

Values have explicit tags:

```json
{"t":"number","n":"1","d":"3"}
{"t":"quantity","n":"1","d":"3","u":"q"}
{"t":"ref","path":["bass_synth","params","level"],"port":null}
{"t":"symbol","v":"C4"}
{"t":"string","v":"Chorus"}
```

Lists and tuples remain distinct tagged values with ordered `items`. Records use an unordered `fields` dictionary. Booleans have a boolean payload. Calls have a constructor name and ordered arguments. Integer strings have no plus sign or leading zeros, and canonical fractions are reduced with positive denominator. The schema constrains their shape; arithmetic reduction and reference/type rules require semantic validation.

### 20.2 Semantic normalization

For revision and editing, the source has a canonical authored typed graph
**A**. It preserves authored object and child IDs, reference paths, labels,
field maps (including absent fields and explicitly empty maps), typed unit tags,
symbols, and constructor forms. Rational values in **A** use reduced
numerator/denominator form. Text comments, insignificant whitespace, and other
surface formatting are outside the canonical trees and excluded from canonical
bytes, but no semantic authored distinction is silently removed.

Semantic normalization computes a separate execution view **N(A)**. It must
resolve references, validate field types, expand specified defaults, and
canonicalize execution units (`ms` to s, kHz to Hz) without mutating **A** or
silently materializing those defaults or lowering constructors into **A**.
Pitch letter tokens lower to `key(k)`; `ratio` lowers to an exact Hz quantity;
`bar` lowers to q. Explicit named tuning degree references remain because their
source identity is meaningful. Comments and other surface formatting may be
retained outside the canonical trees but are excluded from the execution hash.

The source graph remains compact: uses and placements are not flattened for canonical source storage. A separate performance digest may describe an expanded performance. These are different hashes and must not be confused.

Canonical JSON for **A** uses UTF-8, no insignificant whitespace, no BOM, no
final newline, object keys sorted by Unicode code-point order, and shortest
unescaped Unicode strings except that double quote, backslash, and U+0000–U+001F
require JSON escaping. Control characters use lowercase `\u00xx` rather than
optional short escapes. Slash is not escaped. Numbers with semantic precision
are encoded in the tagged rational forms, never as JSON floating-point
numbers. The typed syntax-tree root `version` is the literal JSON integer 1.
Surrogate code points are
forbidden. The **revision hash** is SHA-256 of these canonical authored **A**
JSON bytes, including authored labels and declared external asset hashes but
excluding non-source UI sidecars and the revision hash itself. Protocol 2
describes this algorithm context as `maac.revision.authored.sha256/1`; that
identifier is metadata only. The digest has no extra prefix bytes and
incorporates no hidden schema or normalization state.

The **execution hash** is SHA-256 of canonical JSON bytes for the normalized
execution graph **N(A)** after removing the optional `label` entry in an actual
source object's `fields` map and excluding nonexecuting extension data
identified by an understood explicit schema. It uses the same canonical JSON
encoding defined above for **A**, with no extra prefix bytes. While traversing
source objects and their actual children, this label exclusion preserves
`label` fields nested in params, config, or other records (including
`extension.data`), object IDs named `label`, and `override.set.label`. There is
no name-based recursion through record values; any additional omission requires
an understood schema's explicit nonexecuting-data rule. Renaming an ID may
change these hashes because identity can affect randomness and deterministic
reduction order. A hash is not a claim that two different source graphs cannot
happen to sound the same.

The **render key** additionally covers the execution hash, all transitive dependencies, processor/adapter versions, state hashes, sample rate, numerical mode, render window, tail policy, block schedule where applicable, and output-encoding settings. There are no self-referential project hashes embedded in the object graph.

## 21. Exact editing protocol

Text is one view of the document; editing actions operate on typed object addresses. A patch transaction carries a base revision, explicit operations, and optional preconditions:

```json
{
  "version": 2,
  "base_revision": "sha256:<actual source revision>",
  "operations": [
    {
      "op": "set",
      "object": ["bass_riff", "n2"],
      "field": ["pitch"],
      "expect": "G2",
      "value": "A2"
    }
  ]
}
```

This example uses source-value strings for readability. The wire representation
of `expect` and `value` is the tagged syntax-value representation. Protocol 2
is versioned independently of the `maac 1` language and syntax-tree version 1.
A receiver that requires protocol 2 rejects a version-1 transaction with
`E_CAPABILITY` before checking its base revision or mutating the document; it
must not guess or convert a base revision computed under another revision
authority. A production transaction must
supply a real hash. The transport encoding is JSON and never executes an
`op` string as code. Expectations use semantic equality in the fixed base
context, including labels; they do not use audible equivalence. Defined
normalization compares `C4` and `key(60)` equal in a pitch context, and `20ms`
and `1/50s` equal in a duration context. Distinct pitches that happen to
produce the same frequency do not collapse.

Core operations are:

- `set`: replace an existing or optional field at an object/record path, with an optional expected old value; cannot directly rename an ID or object kind.
- `unset`: remove an optional field, restoring its specified default; removing a required field fails validation.
- `insert_object`: add a complete typed object at an explicitly named parent and unused ID.
- `delete_object`: remove a named object; dangling references fail validation unless handled elsewhere in the same transaction.
- `rename_id`: change an object's ID and rewrite all source references,
  instance override addresses, and declared structural mappings in the
  candidate. It MUST NOT rewrite any remaining operation target paths or
  payloads (`expect`, `expect_object`, `value`, or `object_value`). Changes to
  randomness/hash identity must be reported.

Ordered lists are replaced as fields in core patches. A UI may implement fine-grained breakpoint edits internally, but it must check the expected old list or revision rather than blindly rely on a stale list index. Core has no query-text execution, wildcard mutation, or implicit last-writer-wins merge.

Before operation evaluation, the supplied `base_revision` must equal
`SHA-256(canonical A0)` for the immutable original authored graph **A0**;
otherwise the receiver reports `E_CONFLICT` and does not mutate. All operations
are then processed in listed order on a private candidate derived from **A0**.
For each operation, the receiver resolves its target against the current
candidate, evaluates its preconditions against **A0** through the surviving
identity correspondence, and then applies the operation privately. A rename
therefore does not implicitly rewrite any remaining operation's target or
payload: a later operation must spell the candidate path it intends to address.
An existing object retains its base correspondence through renames and through
field or record replacement. Deleting an object breaks the base correspondence
for it and all descendants; reinserting the same ID creates a new identity. A
base precondition cannot target a newly inserted object or a descendant without
base correspondence. Candidate mutation requires every intermediate record to
exist and never synthesizes missing records. Only after every operation and
final semantic validation succeeds is the private candidate committed
atomically. Failure discards the private candidate and changes nothing. A patch
result includes the new revision, inverse patch, affected source IDs, affected
expanded event addresses or a bounded summary, render-invalidated
regions/dependencies, and diagnostics. Inverse patches include removed values
and full removed objects.

Query and transform commands such as “halve density” are tooling, not execution semantics. They must resolve to explicit targets and changes before commit. A source edit and an occurrence override are different actions. Tools must expose that choice, especially when a source note has many instances.

Optimistic concurrency uses the base revision plus optional field expectations. Conflicting concurrent edits are reported rather than musically “blended.” A tool may rebase disjoint field edits, but must recompute validation and impact. A merge that introduces a second automation writer, a dangling event override, or a graph loop fails.

### 21.1 Wire operation shapes

The transaction object has exactly `version`, `base_revision`, and
`operations`; unknown fields are rejected. Each operation object also rejects
unknown fields. Object and field paths are nonempty arrays of source identifier
strings. The empty `parent` path denotes the document root. The base revision
is a string matching `sha256:` followed by exactly 64 lowercase hexadecimal
digits. `version` is the integer 2, and `operations` is an ordered nonempty
array.

The exact operation fields are:

| Operation | Required fields besides `op` | Optional fields |
|---|---|---|
| `set` | `object`, `field`, `value` | `expect`, `expect_absent` |
| `unset` | `object`, `field` | `expect` |
| `insert_object` | `parent`, `id`, `object_value` | None |
| `delete_object` | `object` | `expect_object` |
| `rename_id` | `object`, `new_id` | None |

`value` and `expect` are tagged syntax values. `set` and `insert_object` store
the supplied authored forms in **A**, after rational scalar canonicalization;
they do not expand defaults, convert units, or lower constructors during the
write. `expect_absent`, when present,
must be true and cannot coexist with `expect`; it checks authored field
presence, so a field supplied only by a default is still absent. `object_value`
and `expect_object` have the typed object shape from section 20, excluding an
ID because the containing dictionary owns that ID. `field` descends through
named records, not through lists or child objects; child objects are addressed
in `object`. A `set` can add an optional field but must not synthesize missing
intermediate records. `unset` requires the field to exist in the authored
candidate. If a base record on an expected descendant is missing,
`expect_absent` sees that descendant as absent; a non-record intermediate is an
invalid path. Candidate mutation still requires every intermediate record. A
base `expect`, `expect_absent`, or `expect_object` on a newly inserted object,
including an object deleted and reinserted under the same ID, fails with
`E_CONFLICT`. An absent optional field with a specified base default is
compared to that default for `expect`; an absent field without a specified
default fails with `E_CONFLICT`. Effective-value comparisons use **N(A)** in
the fixed base type, meter, name, dependency, and label context, not audible
equivalence; **N(A)** retains labels and only the execution-hash projection
removes the scoped labels. A failed base or field precondition is `E_CONFLICT`;
a missing target object or missing/non-record intermediate candidate record is
`E_REFERENCE`, as is `unset` of an absent authored leaf field; a negative
`tail` remains an ordinary semantic `E_RANGE`.

A delete-object expectation compares the complete normalized object subtree. It is not an arbitrary expression or partial pattern. A rename requires an unused sibling ID. Core operations cannot directly change a processor's opaque internal state: changing `node.state` explicitly selects a different identified state asset and is validated through that processor's adapter.

For inverse generation, the transaction records the original canonical
authored tree and its identity mapping. The inverse restores that tree,
including field omissions, typed units, and constructor forms; it does not
promise to restore source comments or formatting. Its `base_revision` is the
new final revision. Optional guards in an inverse are either omitted or are
evaluated against that final base, never copied from intermediate prior
values. A transaction that renames an object and then changes its pitch must
spell the renamed candidate path in the second operation; the rename does not
rewrite that operation's target or expectation.

### 21.2 Normative examples

- `core.constant/1` defines optional `params.value` with default `0`. In a
  base with no authored `params` record, `expect_absent: true` for
  `params.value` succeeds because the descendant is absent, while `expect: 0`
  uses the specified default. A candidate `set` or `unset` cannot synthesize
  the missing `params` record and therefore fails with `E_REFERENCE`. After an
  explicit `params = {}` record is authored, a later `set` may add the absent
  `params.value` leaf, while `unset` still requires that leaf itself to be
  authored. `expect_absent: true` always checks authored presence.
- Authored `20ms` and `1/50s` are equal in **N(A)** after defined duration
  normalization while remaining distinct in **A** and therefore in the
  revision. The authored unit requested by a `set` is preserved.
- A project with omitted `tail` and an otherwise identical project with
  authored `tail = 0s` MUST have different revisions and MUST have equal
  execution hashes when all other source and dependency data are equal.
- In a 4/4 meter, an explicit edit may replace `bar(2, 1)` with `4q` in **A**.
  A later meter change does not move that explicit `4q`; an unedited `bar`
  remains meter-relative in **N(A)**.
- A transaction can rename `bass_riff` to `lead_riff` and then target
  `lead_riff` only if the second operation spells that candidate path. The
  engine does not rewrite the later path or its `expect` payload. The renamed
  existing object retains its base correspondence.
- If a patch changes an absent `tail` to authored `0s`, its inverse uses the
  resulting revision as `base_revision` and restores the absent field from the
  original canonical **A**. It does not guard on an intermediate prior value.
- Protocol 2 names the revision algorithm context
  `maac.revision.authored.sha256/1`; this descriptive identifier adds no bytes
  to the SHA-256 input. A v2-only receiver rejects a version-1 transaction with
  `E_CAPABILITY` before checking its base or mutating the document.

## 22. Dependency locks, execution, and reproducibility

A package may contain:

```text
song.maac
maac.lock.json
assets/<content-addressed files>
modules/<content-addressed permitted modules>
ui.json
renders/<artifacts and manifests>
```

The lock manifest has `version`, `execution_hash`, `assets`, `processors`, `engine`, and `output` objects. Each asset entry records source ID, exact SHA-256, and verified byte length. Each processor entry records node ID, type, implementation and descriptor hashes when external, adapter identity, state hash, config digest, declared latency, and determinism status. Engine records implementation/build identity, platform/architecture, numerical mode, sample rate, and any relevant block schedule. Output records encoding, channel order, crop, tail, clipping/dither policy, and expected PCM hash when asserting exact reproducibility.

Three claims must be kept separate:

**Source equivalence:** same normalized document meaning and dependencies.

**Performance equivalence:** same resolved note/trigger schedules, parameter functions, and processing topology at the declared timing precision.

**Audio equivalence:** either a declared numerical error bound against reference samples, or byte-identical PCM under an identified execution environment. Container bytes may additionally differ because of headers or metadata; a PCM hash and a file hash are distinct.

A seed alone does not establish audio equivalence. A native plug-in might depend on internal random state, thread scheduling, CPU math, block size, device inputs, or unavailable assets. A declared deterministic descriptor is a contract, not empirical proof; a locked render should include repeated-render tests. Bitwise claims require verified matching PCM, not just matching version strings.

When a processor cannot be reproduced, a freeze operation records its output as an immutable audio asset at an explicit graph boundary, preserves its original source graph, and marks which path is active. Freeze manifests include the exact input-event/automation/dependency hash, engine configuration, start state, time interval, latency, and tail. Editing an upstream dependency invalidates the freeze. A bounced stem is not automatically equivalent to a live node when downstream sidechains, sends, nonlinear summing, or a different render range change the context.

The core execution path performs no automatic gain staging, mastering, denoising, normalization, clipping, limiting, sample-rate substitution, or missing-device substitution. Monitoring safety attenuation may exist outside the render path and must be visibly separate from the composition.

## 23. Validation and diagnostics

Validation proceeds through lexical parsing, object/schema validation, reference resolution, dimensional typing, temporal validation, pattern-DAG validation, instance expansion or bounded analysis, destination-capability checks, automation-writer checks, graph-causality checks, dependency verification, and render-profile checks.

Errors must contain a stable code, source object path, relevant field path, source span when available, and an actionable message. A diagnostic may suggest a correction but must not silently apply it.

| Code | Required condition |
|---|---|
| `E_SYNTAX` | Invalid grammar or token |
| `E_DUPLICATE_ID` / `E_DUPLICATE_FIELD` | Ambiguous source identity or field |
| `E_UNKNOWN_FIELD` / `E_UNKNOWN_KIND` | Unrecognized core data |
| `E_REFERENCE` | Missing or wrong-kind target |
| `E_UNIT` | Missing or incompatible unit |
| `E_RANGE` | Value outside a declared valid range |
| `E_TEMPO` | Nonpositive tempo, bad shape, unordered points |
| `E_METER_BOUNDARY` | Meter change not on an existing bar boundary |
| `E_INTERVAL` | Invalid source span, onset, or effective gate |
| `E_TIME_PRECISION` | Scheduling boundary cannot be established |
| `E_SUBSAMPLE_NOTE` | Positive gate collapses to zero engine frames |
| `E_PATTERN_CYCLE` | Recursive pattern-reference graph |
| `E_INSTANCE_TARGET` | Override references a missing occurrence |
| `E_AUTOMATION_WRITER` | Multiple replacement lanes on one parameter |
| `E_CAPABILITY` | Receiver, processor, or extension cannot execute declared semantics |
| `E_PORT_TYPE` | Incompatible port types, channels, or cardinality |
| `E_ALGEBRAIC_LOOP` | Same-sample dependency cycle |
| `E_ASSET` / `E_HASH` | Missing, invalid, or identity-mismatched dependency |
| `E_VOICE_LIMIT` | Explicit voice capacity exceeded |
| `E_RENDER_STATE` | Incompatible initialization or restored state |
| `E_NONFINITE` | Nonfinite source audio or computed DSP result |
| `E_CONFLICT` | Patch precondition or concurrency conflict |
| `E_RESOURCE_LIMIT` | Declared host limit exceeded without changing document meaning |

Warnings are appropriate for possible clipping, inaudible notes, duplicate unison voices, unequal parallel-path latency, unused objects, and unexpectedly large linked-edit impact. Those conditions are not necessarily errors or invitations for automatic correction.

Each implementation publishes hard resource limits: file bytes, object count, rational numerator/denominator bit length, expansion count, nesting depth, asset bytes, channels, voices, and processing budget. It must report `E_RESOURCE_LIMIT` rather than silently dropping notes, reducing precision, or changing the graph. A conforming finite expansion may be computed lazily; finiteness does not require loading an entire album's events into memory.

## 24. Security and execution boundaries

Parsing and inspecting a source file must not execute a module. Imports are offline content resolution, not `eval`. Modules receive only explicitly permitted asset handles and declared host services. Native execution requires host/user trust authorization independently of document validity. An AI-generated source document is untrusted input.

Core documents cannot send email, control a browser, launch commands, write outside the project, or trigger an arbitrary HTTP request. Message bytes are musical receiver data, not permission to perform external actions. Live device input and networking belong to separately permissioned host sessions; captured results become explicit immutable audio or events.

Assets, descriptors, source IDs, patch payloads, and processor metadata all require length and resource limits. Archive import must defend against traversal, symbolic-link escape, duplicate path ambiguity, decompression bombs, and mismatched hashes. Permission refusal is an execution error, not a reason to silently substitute output.

## 25. Interchange and explicit loss reporting

MIDI, notation, DAW-session formats, and audio exports are adapters, not the canonical model. An adapter returns an output artifact and a machine-readable loss report. Each loss identifies source object paths, affected property, output limitation, and chosen approximation or omission. Faithful mode refuses an export that requires an unapproved lossy decision.

A MIDI adapter must report unsupported microtonal pitch, overlapping same-key identity problems, per-note expression approximations, tempo-ramp discretization, timing-resolution loss, audio omission, and absent synthesis/routing state when relevant. It must state the target MIDI version/profile rather than saying merely “MIDI compatible.”

A notation adapter must distinguish sounding duration from spelling, ties, voices, tuplets, and engraving. Core does not reconstruct the composer's preferred notation uniquely from sound. A DAW-project adapter preserves whatever session state the target actually supports; it must not claim that a generic filter setting recreates a different DSP implementation's waveform.

Rendered audio preserves a particular result, not the editable note/graph structure that produced it. An audio import cannot invent a lossless symbolic transcription. Original recorded assets remain first-class source even when no reliable transcription exists.

## 26. Versioning and extensions

`maac 1;` selects this language's core semantics. A future change that alters the interpretation of valid source requires a new major language version or a distinct explicitly required capability. New named core processor versions never replace old processor behavior silently: `core.onepole/2` is a different type from `/1`.

The exact editing protocol is versioned independently of both `maac 1` and the
typed syntax-tree version 1. Protocol 2 uses the authored revision algorithm
context `maac.revision.authored.sha256/1`; it does not alter retained-plan
formats or current production/import identities. A receiver requiring protocol
2 rejects a version-1 transaction with `E_CAPABILITY` before checking its base
or mutating the document and must not silently reinterpret or convert its base
hash.

An `extension` requires `namespace`, an exact versioned identifier string; `schema`, a descriptor-asset reference; `render_affecting`, boolean; and `data`, a record. Its namespace must appear in `project.requires`. A host that does not understand a required extension may preserve it for Document-only inspection but must not claim semantic normalization or faithful rendering of that document. In particular, it must not trust an unknown schema's assertion that arbitrary data is non-rendering merely to omit it from a hash.

Extensions may add explicitly namespaced object structures or capabilities only through their published schemas and adapters. They cannot redefine core units, mutate another object's behavior implicitly, shadow core IDs, or grant execution permission. Compatibility is demonstrated by conformance tests, not inferred from a shared filename extension.

The [production extension](docs/production.md) applies these rules to
`maac.production/1`. Its delivery data remains in the execution hash;
delivery/render identity additionally records the selected targets and the
encoding, resampler, dither, and analyzer identities defined by that contract.

---

## 27. Complete worked source

The companion `example.maac` is a self-contained 8-bar, 4/4, 120 BPM Core Audio document. It uses only reference sine instruments and contains no external assets. Its main score is 32q (16 seconds), followed by a one-second tail. There are 48 note occurrences before any expansion of tooling views: 24 bass notes, 16 upper notes, and 8 pulse notes.

The source includes a shared bass riff, an occurrence-only 20ms timing edit, a two-note motif, a one-note pulse pattern, an automated one-pole cutoff, mono-to-stereo pan nodes, and explicit summing. The sound design is intentionally simple so that representation and routing can be inspected without a proprietary synthesizer.

The editing example in section 21 changes a shared source note and demonstrates a different scope from the occurrence override in the worked source. The bundle does not include an audio renderer; the worked source is a language example, not an assertion that an installed program can already play it.

## 28. Conformance vectors

The companion `conformance.json` stores concrete vectors; `check_spec.py` checks the grammar against the worked source and runs a selected set of exact arithmetic and scheduling assertions. It is a **syntax and selected-semantics smoke checker**, not a complete semantic validator, renderer, security sandbox, or proof of this specification's consistency.

Required cases for a full implementation include:

1. At 120 BPM, `1q` is exactly 0.5 seconds; `1/3q` is exactly 1/6 second and 8000 frames at 48000 Hz.
2. Three durations of `1/3q` add to exactly 1q, without cumulative floating-point drift.
3. `1.5kHz` normalizes to 1500Hz; `20ms` normalizes to 1/50s.
4. In 6/8, bar 2 starts at 3q and `bar(2,4)` is 9/2q.
5. At 120 BPM, a note at 4q for 1q with both offsets 20ms has on/off times 2.02s and 2.52s: frames 96960 and 120960.
6. The same note with only onset shifted has a shorter gate; no implicit duration preservation is allowed.
7. A linear 120→180 BPM ramp over 4q lasts `4*ln(3/2)` seconds, not a linear interpolation of seconds-per-beat endpoints.
8. The step map `(0q,120bpm)` then `(4q,60bpm)` places 6q at 4 seconds.
9. Two same-key notes with different IDs remain distinct; the first one's off event must not stop the second voice.
10. A source note at 3q for 2q in a 4q pattern ends at 5q under spill, but at 4q under cut.
11. For length 4q, count 2, stretch 3/2, the repetition origins are 0q and 6q.
12. A placement-local override changes one occurrence, not its source or sibling repetitions.
13. Reordering declarations does not reorder event identity or audio summation; changing explicit IDs may.
14. Exponential interpolation from 500Hz to 2000Hz has midpoint 1000Hz; linear dB interpolation from -12 to 0 has midpoint -6dB.
15. Duplicate automation writers, dangling overrides, zero-delay graph cycles, and absent asset bytes each produce their designated errors.
16. A graph feedback loop with an explicit one-frame delay is causal; a cycle with only an opaque reported-latency plug-in is not automatically accepted.
17. A seek render with proper prehistory equals the matching full-render crop; a reset-at-crop shortcut must not pass this test.
18. A byte-identical source with an altered processor binary fails lock verification.
19. Noise samples are invariant to adding an unrelated node because random addressing is node-local.
20. A sub-sample positive gate that maps both endpoints to one frame fails rather than being arbitrarily lengthened.

## 29. Design rationale and relationship to existing work — informative

This proposal should not be read as a claim that Strudel cannot arrange a whole piece. Its official pattern constructors include concatenation, layering, arrangement, and polymeter. The distinction here is the priority given to a persistent typed project graph, stable occurrence addresses, closed execution dependencies, and transactional editing. [R1]

Tidal's rational-time representation is a useful precedent for exact musical subdivisions. MaaC adopts exact rational score time, but separates it explicitly from physical time and output frames and limits its core pattern structures to finite declared composition. [R2]

DAWproject is an important existing reference for the breadth of session information: notes, expressions, audio, automation, plug-in state, and surrounding project structure. MaaC is not claiming to invent the need for a production-state interchange model. Its proposed differences are a source-oriented language, strict execution contracts, and an editing/identity protocol. [R3]

CLAP provides relevant precedents for sample-addressed events, note expression, stable parameter identities, and distinct parameter and non-parameter state. Those precedents inform the separation between native performance events, parameter automation, processor descriptors, and opaque state. A MaaC CLAP adapter would still need a complete implementation and fidelity tests. [R4–R6]

MusicXML is a notation interchange reference. A notation representation and a fully resolved performance are related but different layers; the proposal does not assume one can be reconstructed uniquely from the other. [R7]

The central trade-off is intentional: a small closed declarative core sacrifices arbitrary in-document programming in exchange for inspectability and predictable editing. External programs can generate arbitrary finite content; a future generator profile can preserve additional algorithms, but its complexity should not leak into the meaning of a simple note or audio clip.

## 30. Reference sources — informative

All references were inspected on 7 September 2026. They describe existing systems, not endorsements of or conformance with MaaC.

- **R1.** Strudel, “Creating Patterns,” including arrange, stack, and polymeter. `https://strudel.cc/learn/factories/`
- **R2.** Tidal Cycles, “What is a pattern?”, rational time and time-to-events queries. `https://tidalcycles.org/docs/innards/what_is_a_pattern/`
- **R3.** Bitwig/PreSonus DAWproject repository and format description. `https://github.com/bitwig/dawproject`
- **R4.** CLAP event definitions. `https://github.com/free-audio/clap/blob/main/include/clap/events.h`
- **R5.** CLAP parameter extension contract. `https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/params.h`
- **R6.** CLAP state extension contract. `https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/state.h`
- **R7.** MusicXML 4.0 specification. `https://www.w3.org/2021/06/musicxml40/`
