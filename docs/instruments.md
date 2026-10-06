# Reusable instruments and sound libraries

This document specifies the local library extension to MaaC/1 for reusable
musical data and sound definitions. It extends
the implemented foundation without changing existing `core.* /1` processors,
exact event scheduling, or version 1 performance-plan rendering. The user-facing
delivery is reusable source files and offline 48 kHz mono/stereo audio.

The [plucked-string implementation contract](plucked-string.md) separately
specifies `synth.pluck/1` and `std/acoustic/1.0.0`. Its design is approved;
implementation and acceptance evidence are separate. It preserves existing
processors, graph DAG rules, and frozen basic-library bytes.

## Documents and dependencies

A library MUST have exactly one `library` declaration and no `project`.
A composition has one `project` and no `library`. In addition to its `library`
declaration, a library permits only top-level `import`, `instrument`, `preset`,
`wavetable`, `sample`, `pattern`, `curve`, and `tuning` declarations. The seven definition
kinds are directly exported by their authored IDs; `library` and `import`
declarations are metadata and dependency bindings, not exports. Child objects
retain the rules of their enclosing definition kind. Compositions may declare
these reusable definitions alongside their existing composition objects.

Every export MUST be validated, including unused exports. The generic MaaC
surface grammar already supports these declarations; this inventory does not
add syntax or a new export declaration. A library import MUST resolve to a
library document, not a composition.

```maac
maac 1;
library studio { version = "1.0.0"; creator = "Example author"; license = "Apache-2.0"; }
import colors { path = "colors.maac"; hash = "sha256:<64 lowercase hex digits>"; }
```

The library version is a nonempty metadata string; `maac 1` remains the language
version. Creator and license are optional strings, preserved as provenance;
empty optional strings are allowed. Each metadata value is limited to 4096
UTF-8 bytes.
Imports name an explicit alias and pin the exact UTF-8 file bytes. A reference
to an export is `&name` in its declaring document or `&alias.name` through a
direct import. Transitive imports remain relative to their declaring files;
aliases do not leak across documents. Re-exporting and wildcard imports are
outside this extension.

An import selects exactly one of two forms: a local `path` plus `hash`, or an
exact `builtin` identity. Mixing those fields, adding unknown fields, or using
an unavailable built-in version fails explicitly.

```maac
import basic { builtin = "std/basic/1.0.0"; }
```

The [basic collection](basic-instruments.md) contains 24 stereo synthesized
instruments embedded into the executable. It needs no filesystem sound library
or assets. Its exact released source bytes are frozen per version; changed
sound graphs, defaults or seeds require a new version. The resolved source
identity is `@builtin/std/basic/1.0.0.maac`, with a SHA-256 calculated from its
UTF-8 bytes and retained in source/dependency provenance. `@builtin/` is a
reserved namespace; callers cannot replace it through source/asset maps or
local import paths. Multiple aliases reuse the same source document. Embedded
sources count toward the same file, byte, syntax-object and import-depth limits
as local sources. Local and built-in imports can coexist, including built-in
imports declared inside a pinned local library. No version alias, fallback,
network lookup or filesystem shadowing occurs.

Paths are normalized project-relative POSIX paths. Absolute paths, backslashes,
root escapes, import cycles, missing files, duplicate identities, malformed
pins, and hash mismatches fail explicitly. The filesystem loader pins the
caller-selected project root as an open directory capability. It resolves
contained symlinks, opens files relative to that capability, checks the opened
handle is a regular file, and reads through the same handle. A changed path
component cannot redirect a contained open outside the root. CLI source
commands use the entry file's directory as the project root unless an explicit
project root is supplied. No network
resolution occurs. Path keys and authored references are bounded to 4096 UTF-8
bytes before normalization; normalized paths have the same limit.

## Reusable musical declarations

Exported `pattern`, `curve`, and `tuning` objects retain their existing core
fields, child objects, units, defaults, and validation rules. Patterns retain
finite nesting, musical coordinates, transforms, and explicit curve/tuning
references. Curves retain their declared clock, coordinates, values, and
interpolation. Tunings retain their explicit definition and reference pitch.
The library extension adds no pattern parameters or implicit transformations.

### Reference ownership and caller context

A reference to a top-level export uses `&name` in its declaring document or
`&alias.name` through an import declared in that document. This rule also
applies inside exported patterns: nested `use.pattern` references,
note-expression curve references, and explicit tuning references in `degree`
resolve in the definition's own document and imports. Caller declarations MUST
NOT satisfy a missing library reference. The referenced export MUST have the
kind required by the consuming field.

Transitive dependencies resolve through each declaring document's imports.
Their aliases do not become caller aliases: a caller MUST NOT traverse an
import chain using `&outer.inner.name`. A directly exported pattern may use a
dependency's pattern without re-exporting the dependency's declarations.
This does not restrict the existing child/port/parameter reference forms in
fields that explicitly accept them; it defines access to top-level exports.

The consuming composition supplies its project tempo and meter, track
destinations, placements and placement transforms, and global automation
targets and anchors. Existing composition rules determine how imported
material is scheduled and consumed. Importing a definition MUST NOT replace
those choices or introduce implicit bindings. Note-expression attachments and
tuning references authored within a library remain owned by that library.
Libraries do not declare top-level tempo, meter, track, placement, routing, or
global automation objects.

### Validation boundary

Validation MUST cover every export in the entry and its reachable library
dependencies, including unused exports. Reusable declarations must satisfy
their own schema, reference, type, unit, range, and acyclic-pattern requirements
without inventing a project clock or destination. When a composition consumes
them, its placement, timing, target, and receiver requirements are also checked.
A successful library check does not imply compatibility with every receiver.
A library can be checked independently; compiling a performance requires a
composition.

### Source identity and derived identity

An imported event's source mapping preserves the import-alias route to its
declaring pattern and leaf. Its occurrence address remains the placement ID,
repetition indices, nested `use` IDs, and leaf ID defined by
[MaaC-1 §10](../MaaC-1-Specification.md#10-tracks-placement-and-event-identity).
For example, a source path `["music", "notes", "leaf_note", "tone"]` identifies
the declaring leaf through two import aliases; its expanded occurrence may
be `play/0/nested/0/tone`. The source path does not grant a caller the authored
reference form `&music.notes.leaf_note`.

Resolution and compilation MUST NOT rewrite the canonical authored graph
**A**. Authored IDs, import declarations and pins, and reference paths retain
the revision and editing rules of
[MaaC-1 §§20–21](../MaaC-1-Specification.md#20-canonical-data-and-hashes).
Internal resolved catalogs and expanded performance events are derived data.
Execution hashes retain the existing §20.2 **N(A)** definition; render keys
retain their transitive dependency requirements. This extension defines no
new hash preimage and promises no identity preservation after alias renaming.

An import pin hashes the exact UTF-8 source bytes. An authored revision hashes
the canonical authored document under §20.2. The separate module-artifact
digest hashes its canonical package JSON. These identities MUST NOT be
substituted for one another.

### Musical source packaging

The [module artifact contract](musical-module-artifact.md) packages a library
entry and its exact reachable source and asset closure as
`maac.module-source/1`. Its entry must directly declare at least one pattern,
curve, or tuning. This packaging condition does not require every valid
library to contain a musical export: a valid empty, instrument-only, or
import-only library can be ineligible for this artifact format. The artifact's
musical export inventory contains only direct entry declarations.

Artifact export/check/unpack preserves ordinary source imports. Unpacked
libraries are consumed through existing path/hash declarations; no direct
artifact import syntax is introduced. The
[contract validation record](musical-library-contract-validation.md) identifies
the fixed semantic cases and their execution evidence separately from these
normative rules.

## Instruments, controls, and presets

```maac
instrument fm_bell {
  channels = 1;
  voice v {
    channels = 1;
    amplitude = &amp;
    output = &carrier:out;
    node amp {
      type = "synth.adsr/1";
      params = { attack = 2ms; decay = 400ms; sustain = 0; release = 300ms; };
    }
    node modulator { type = "synth.sine/1"; params = { ratio = 2; }; }
    node carrier { type = "synth.sine/1"; }
    modulate fm { from = &modulator:out; to = &carrier.params.frequency; depth = 250Hz; }
  }
  control release { target = &v.amp.params.release; default = 300ms; }
}
preset soft_bell { instrument = &fm_bell; params = { release = 600ms; }; }
```

An instrument has an explicit `channels` field (1 or 2), one `voice` graph, and
optionally one `shared` graph. Graph names are local to their instrument.
Controls are named children; `target` identifies a graph node parameter and
`default` is required. A control retains the target parameter's units, range,
and rate; it cannot change them. Two controls cannot expose the same target.

Each graph declares its output channel count and `output = &node:out`. The voice
graph requires `amplitude = &adsr_node`; shared graphs have no amplitude ADSR.
Shared graphs receive the stable sum of completed voice outputs at the reserved
`&input:out` source, whose channels equal the voice graph's output. They may use
gain, low-pass, high-pass or resonant filtering, saturation, mixing, panning, and LFO modulation. They run once per
sample through the project tail, including when no voices remain. Without a
shared graph, voice and instrument channel counts must agree; otherwise shared
and instrument channel counts agree. Graph nodes cannot use the reserved ID
`input`.

```maac
node lead {
  instrument = &sounds.fm_bell;
  preset = &sounds.soft_bell;
  config = { voices = 64; };
  params = { release = 600ms; };
}
track melody { target = &lead:events; }
```

Instrument instances expose `events` and `out`. A node selects either a core
processor `type` or an `instrument`, never both. Presets must select the same
instrument identity as the instance. Settings apply as instrument control
defaults, then preset values, then instance values. The default voice capacity
is 64. Existing automation targets public controls as
`&lead.params.release`; internal node paths are private. Definition data and
all mutable instance/voice states are independent.

## Kits

A kit is an instrument whose voices are other instruments, selected by `hit`
keys. A declaration with `piece` children is a kit.

```maac
instrument boom_bap {
  channels = 2;
  piece kick  { instrument = &basic.kick;       key = "kick";  gate = 100ms; }
  piece snare { instrument = &basic.snare;      key = "snare"; gate = 100ms; params = { brightness = 4000Hz; }; }
  piece hat   { instrument = &basic.closed_hat; key = "hat";   gate = 35ms; choke = "hats"; }
  piece open  { instrument = &basic.open_hat;   key = "open";  gate = 220ms; choke = "hats"; }
  control kick_level { target = &kick.params.level; default = 0.25; }
}
node drums { instrument = &boom_bap; params = { kick_level = 0.3; }; }
track beat { target = &drums:events; }
```

**Declaration.**
- A kit has `channels` (1 or 2), one or more `piece` children, and optional
  `control` children. A `voice` or `shared` graph is `E_UNKNOWN_KIND`.
- Each piece has:
  - a required `instrument` reference to an instrument export (a kit is
    `E_REFERENCE`), with optional `preset` and `params` that set that
    instrument's public controls as an instance would;
  - a required nonempty string `key`, unique within the kit;
  - a required physical `gate` in seconds, positive and at most 60 s;
  - an optional `pitch`: a spelled pitch, `key(k)`, or a frequency, default
    `C4`;
  - an optional integer `voices` from 1 through 4096, default 8;
  - an optional nonempty `choke` group string.
- A piece's instrument must have the kit's channel count, and a kit holds at
  most 64 pieces.
- A kit `control` has `target = &piece.params.control` and a required
  `default`. It takes that piece control's unit, range, and rate, exposes it
  on the kit instance, and is unique per target. It cannot expose a control
  the piece already sets in `params` (`E_CONFLICT`) or a reset-rate control
  (`E_CAPABILITY`). A kit exposes at most 64 controls.

**Instance.** A kit node takes `instrument`, optional `preset`, and `params`
for its controls. It takes no `config`: each piece declares its voices.
Presets, automation, and modulation of kit controls follow the instrument
rules. The node exposes `events` and `out`.

**Playing.**
- The node accepts hits only. A note sent to it is `E_CAPABILITY`, and a hit
  whose key matches no piece is `E_REFERENCE`.
- A hit at frame `n` starts a note on the matching piece's instrument, at the
  piece's pitch with the hit's velocity. The note is released at frame
  `n + ceil(48000*gate)` (the engine rate in general), unless a choke
  releases it first.
- A hit on a piece in a choke group first releases every still-gated voice of
  the group's other pieces at frame `n`. Repeated hits on one piece overlap
  normally. Releases, including choke releases, precede the same frame's hits.
- **Output.** The kit's output is the sum of the pieces' instrument outputs
  in piece ID order. A piece's voices and shared graph behave exactly as an
  instance of its instrument with the piece's settings and exposed controls.
  A kit therefore sounds identical to the same instruments as separate
  nodes, played by notes with the same gates and summed in that order.
- A kit has one output. A piece that needs its own send or processing
  belongs on its own node.

**Plan and limits.**
- Kits travel in the plan's instrument resources as `kits`, with each piece
  naming its instrument program. The instance is a `kit_instrument`
  processor.
- Voice, pluck, and work accounting count every piece. A hit's work bound
  covers its gate plus the largest release its piece can reach.
- Native archives and the generic renderer do not yet support kits and
  refuse them with `E_CAPABILITY`.

**Built-in kit.** `std/basic/1.1.0` adds the `drums` kit; its other exports
and their sound are unchanged from 1.0.0.
- **Keys:** `kick`, `snare`, `clap`, `hat`, `open_hat`, `low_tom`,
  `high_tom`, and `crash`.
- **Gates:** each piece's gate is its drum's example duration. `hat` and
  `open_hat` share the choke group `hats`.
- **Controls:** `<piece>_level`, `<piece>_pan`, and `<piece>_brightness`,
  with defaults equal to the drum's own.

Run `maac instruments drums --library std/basic/1.1.0` for its keys and
controls and a runnable composition.

## Graph processors

Connections use existing `connect { from = &source:out; to = &destination:in; }`
fields and require matching channels. Gain, filters, drive, and pan require one input;
mix accepts zero or more inputs (zero means silence). Every graph must be a DAG
when both audio and modulation edges are considered. Stable ordering uses node
IDs for topological ties and connection/modulation IDs for reductions. Feedback
is rejected, including mixed audio/modulation cycles.

| Versioned processor | Parameters (default; allowed range) | Output |
| --- | --- | --- |
| `synth.sine/1`, `synth.saw/1`, `synth.square/1`, `synth.triangle/1` | `ratio` (1; −64…64), `frequency` (0 Hz; −24000…24000 Hz), `phase` (0; 0…1), `level` (1; 0…16) | Mono |
| `synth.wavetable/1` | Oscillator parameters plus `position` (0; 0…1); required `config.table = &wavetable` | Mono |
| `synth.sample/1` | `ratio`, `frequency`, and `level` as for oscillators (no `phase`); required `config.zones` and optional `config.channels` (1) (see [Samples](#samples)) | Mono, or stereo with `channels = 2` |
| `synth.noise/1` | `level` (1; 0…16); optional nonzero unsigned 32-bit `config.seed` (1831565813) | Mono |
| `synth.adsr/1` | `attack` (0 s), `decay` (0 s), `sustain` (1; 0…1), `release` (0 s); times 0…1800 s; `curve` (0; 0…16) | Mono control signal |
| `synth.timbre/1` | No inputs, parameters, or configuration; voice-only | Mono per-note timbre signal (zero when absent) |
| `synth.pressure/1` | No inputs, parameters, or configuration; voice-only | Mono per-note pressure signal (zero when absent) |
| `synth.velocity/1` | No inputs, parameters, or configuration; voice-only | Mono note velocity, 0…1, constant for the voice's life |
| `synth.key/1` | No inputs, parameters, or configuration; voice-only | Mono base frequency in octaves from C4 |
| `synth.lfo/1` | `frequency` (1 Hz; −200…200 Hz), `phase` (0; 0…1), `level` (1; 0…16) | Mono control signal |
| `synth.gain/1` | `level` (1; 0…16); required `config.channels` | Same channels as input |
| `synth.onepole/1` | `cutoff` (1000 Hz; strictly between 0 and 24000 Hz); required `config.channels` | Same channels as input |
| `synth.highpass/1` | `cutoff` (1000 Hz; strictly between 0 and 24000 Hz); required `config.channels` | Same channels as input |
| `synth.svf/1` | `cutoff` (1000 Hz; 0 up to but not including 24000 Hz), `ratio` (0; 0…64, and 0 in a shared graph), `q` (707/1000; 1/10…40); required `config.channels` and `config.mode` (`lowpass`, `bandpass` or `highpass`) | Same channels as input |
| `synth.drive/1` | `drive` (1; 0…64), `bias` (0; −4…4), `level` (1; 0…16); required `config.channels` | Same channels as input |
| `synth.mix/1` | Required `config.channels`; no parameters | Mono/stereo sum |
| `synth.pan/1` | `pan` (0; −1…1) | Equal-power stereo from mono |

Numbers are dimensionless; seconds accept `s` and `ms`, hertz accept `Hz` and
`kHz`. Parameters and controls use exact rationals until the DSP boundary.
Unknown fields, wrong units, invalid ranges, and nonfinite values fail.
Oscillators and noise are voice-only; high-pass, the resonant filter and the
drive are allowed in both voice and shared graphs. ADSR attack/decay/sustain/curve and oscillator/LFO phase
are captured at note-on (shared LFO phase at render reset). Release is captured
at note-off. Other parameters are sampled each frame. Shared LFO phase controls
accept defaults, presets, and instance values at render reset; they cannot be
automated. ADSRs are voice-only.

`modulate` connects a mono signal to a sample-rate parameter, a voice ADSR's
event-rate parameter, voice oscillator/wavetable/LFO phase, or shared-LFO reset phase. Its required
`depth` has the target parameter's unit, may be signed, and multiplies the
source signal; the result is added to the current parameter value. Multiple
modulations sum in ID order. ADSR attack/decay/sustain capture at note-on and
release captures at note-off from a non-advancing pre-release snapshot.
Voice phase captures at note-on before supplying downstream capture sources.
Shared-LFO phase captures at reset from resolved authored controls, captured
top-level reset-control contributions when present, and silent shared input. Internal event-rate sums are checked only when captured. See [internal event modulation](internal-event-modulation.md)
for ordering, source previews, and conservative release-work bounds. Final evaluated
values must satisfy the processor range; no depth adjustment or clipping occurs.
ADSR, LFO, timbre, pressure, velocity, and key signals may also feed mono audio inputs; stereo signals cannot
modulate a scalar parameter.

### Note sources

`synth.velocity/1` emits the note's velocity, the same value that scales the
voice's amplitude, constant from note-on through release. A kit passes each
hit's velocity to its piece. `synth.key/1` emits `log2(f / 440 Hz) + 3/4`,
the voice's base frequency `f` in octaves from C4 = 440 Hz × 2^(−9/12). `f`
is the frequency the voice receives, including [per-note pitch](instrument-pitch.md)
and before node ratios and frequency offsets. It is evaluated every frame, so
it follows a bend; a note-on capture takes its value at note-on. Under 12-TET,
A4 is 0.75 and A3 is −0.25.

Neither source opts the instrument into an expression kind or changes the
voice path: amplitude still scales by velocity exactly once. They map playing
strength and register to timbre through ordinary connections and modulation:

```maac
node vel { type = "synth.velocity/1"; }
node key { type = "synth.key/1"; }
node amp { type = "synth.adsr/1"; params = { decay = 1200ms; sustain = 0; curve = 7; }; }
node tone { type = "synth.onepole/1"; config = { channels = 1; }; params = { cutoff = 1200Hz; }; }
modulate harder_brighter { from = &vel:out; to = &tone.params.cutoff; depth = 2400Hz; }
modulate higher_shorter { from = &key:out; to = &amp.params.decay; depth = -300ms; }
```

Each mapping is linear and its final value must lie in the target's range:
here a key above 4 octaves from C4 would ask for a negative decay and fail.
The [complete example](../examples/note-sources.maac) applies the same two
mappings to a filtered saw at C3, C4 and C5, each played soft, medium and
hard.

## Timing and audio contract

Every note receives independent oscillator, envelope, filter, and modulation
state. The designated amplitude envelope and note velocity multiply the voice
graph output exactly once. Optional [per-note gain](instrument-gain.md) then
multiplies each output channel before voice summation and shared effects. The
full voice graph still advances at zero gain; gate-end gain holds through release.
Optional [per-note pitch](instrument-pitch.md) changes each voice's base frequency
before node ratios and oscillator frequency offsets. Bends preserve phase,
wavetable state, and plucked-string history; gate-end pitch holds through release.
Live effective-frequency checks still apply at zero gain or velocity.
Optional [per-note timbre](timbre-expression.md) requires an explicit
`synth.timbre/1` node in the voice graph. Its mono output uses ordinary connections
or signed-depth modulation of sample-rate parameters, with final bounds checks.
Each voice evaluates its own curve once per frame and holds its gate-end value
through release; zero gain does not bypass timbre or graph evaluation. No automatic
brightness mapping is inferred. Frozen basic/acoustic graphs do not opt in.
Optional [per-note pressure](pressure-expression.md) independently requires a
`synth.pressure/1` voice node. Its zero-default mono signal uses the same explicit
mapping rules and gate-end release holding, with no automatic amplitude meaning.
Declaring both sources permits pitch, gain, timbre, and pressure on one note.
Source nodes reject any `config`, including an empty record; empty `params`
are allowed. The frozen libraries remain unchanged.
A voice remains allocated until its note-off release
reaches zero, even if its sustain is zero. Voices sum in unsigned UTF-8
event-address order. Overflow is an explicit error. Note-offs and expired-tail
retirement precede note-ons at a shared frame, preserving the existing schedule.

Envelopes are functions evaluated at sample instants. Durations are not
rounded to whole frames. A zero attack immediately reaches 1; a zero decay
immediately reaches sustain. Release starts from the level at note-off; a zero
release retires at that frame. Attack is linear. With `curve = 0`, decay and
release are linear too. A positive curve `c` shapes both by the fraction of
the segment remaining at progress `x` in 0…1:

```text
g(x) = (exp(-c x) - exp(-c)) / (1 - exp(-c))
     = (expm1(-c x) - expm1(-c)) / -expm1(-c)      evaluated form
decay:   sustain + (1 - sustain) g(x)
release: level_at_note_off * g(x)
```

`g` runs from exactly 1 to exactly 0, so a curved decay reaches sustain and a
curved release reaches zero at the same instants as linear ones, and voices
retire on the same frames. Struck and plucked sounds decay roughly
exponentially. With `curve = 7`, a segment is 31 dB down halfway and 61 dB
down at 90 % of its duration. The curve is captured at note-on and applies to
the release too. The score boundary releases still-held notes;
the declared project tail is the render boundary.

Oscillator instantaneous frequency is `note_hz * ratio + frequency`, where
sample-wise linear FM contributes to `frequency`. Output samples the current
phase before advancing by `frequency / 48000`, modulo one cycle. Phase starts
at the declared reset phase; phase 1 equals phase 0. Zero frequency holds phase;
negative frequency reverses it. The supported inclusive frequency range is
−24000…24000 Hz. Harmonic tablebanks select by absolute instantaneous frequency,
without oversampling or interpolation between banks. Bank changes take effect
at the current sample. Cyclic sample interpolation is linear. Basic tables use
2048 samples with harmonic limits at powers of two, standard Fourier amplitudes,
and no peak normalization.

At table phase `p`, each bank sums `c[k] * sin(2*pi*k*p)` in ascending harmonic
order through its harmonic ceiling. The coefficients pin waveform orientation:

| Wave | Coefficient `c[k]` |
| --- | --- |
| Sine / sine LFO | `1` for `k=1`, otherwise `0` |
| Saw | `-2/(pi*k)` |
| Square | `4/(pi*k)` for odd `k`, otherwise `0` |
| Triangle | `8*(-1)^((k-1)/2)/(pi*pi*k*k)` for odd `k`, otherwise `0` |

Gain multiplies each input channel by `level`. One-pole state resets to zero;
with `a = exp(-2*pi*cutoff/48000)`, each channel evaluates
`y = (1-a)*input + a*previous_y`. Panning uses
`theta = (pan+1)*pi/4`, with mono input multiplied by `cos(theta)` on the left
and `sin(theta)` on the right. Mixing adds connected inputs in connection-ID
order without normalization.

`synth.noise/1` has no input and ignores note pitch. Its `level` is sampled
each frame; `seed` is immutable configuration, never a control or automation
target. Every new voice starts an independent unsigned 32-bit state from its
seed; resetting a render restarts the same sequence. With `state` interpreted
as `u32`, each output first advances it in this exact order:

```text
state ^= state << 13
state ^= state >> 17
state ^= state << 5
sample = (state / 2147483648.0 - 1.0) * level
```

Left shifts discard bits beyond 32 bits and right shifts are logical. Output
uses the new state, not the seed itself. Omitted source seeds resolve to
1831565813; explicit seeds must be integers in 1…4294967295. The resolved seed
is stored in the performance plan, so a retained plan does not depend on future
default changes. A missing, zero, out-of-range or noninteger seed in the plan
is invalid. No system entropy or note-address hashing is involved. Same-seed
voices are independent state machines but begin with identical sequences.

`synth.highpass/1` maintains one independent low-pass state per channel,
initially zero. At every sample, with `a = exp(-2*pi*cutoff/48000)`, it computes
`low_new = (1-a)*input + a*low_previous`, outputs `input - low_new`, and stores
`low_new`. It therefore subtracts the newly computed one-pole output, not the
previous state. Mono/stereo channels must match the required `config.channels`.
Cutoff is sampled each frame and must remain strictly between 0 and 24000 Hz,
including after modulation. Its connection rules match the low-pass filter.
Shared filter state continues through the project tail; voice state resets
independently at note-on.

### Resonant filter

`synth.svf/1` is a two-pole state-variable filter in the trapezoidal
(topology-preserving) form, so it stays stable when its cutoff moves every
sample. Its effective cutoff is `note_hz * ratio + cutoff`, the oscillator
convention, and must lie strictly between 0 and 24000 Hz after controls and
modulation; otherwise rendering fails without clamping. A positive `ratio`
keeps the cutoff at the same place relative to each note's harmonics. A
shared graph has no note, so its `ratio` must be 0. `config.mode` is the bare
symbol `lowpass`, `bandpass` or `highpass`:

```maac
node tone {
  type = "synth.svf/1";
  config = { channels = 1; mode = lowpass; };
  params = { cutoff = 200Hz; ratio = 4; q = 2; };
}
```

Each channel keeps two integrator states `s1` and `s2`, initially zero; voice
state resets at note-on and shared state at render reset. Per sample, with
`fc` the effective cutoff, `g = tan(pi * fc / 48000)` and `k = 1/q`:

```text
a1 = 1 / (1 + g (g + k));  a2 = g a1;  a3 = g a2
v3 = x - s2;  v1 = a1 s1 + a2 v3;  v2 = s2 + a2 s1 + a3 v3
s1 = 2 v1 - s1;  s2 = 2 v2 - s2
lowpass = v2;  bandpass = k v1;  highpass = x - k v1 - v2
```

This is exactly the bilinear transform of the analog prototype
`1 / (s^2 + k s + 1)` with the cutoff prewarped, so low-pass and high-pass
slope at 12 dB per octave. At the cutoff, low-pass and high-pass have gain
`q` (3 dB down at q = 0.707), and band-pass has unity gain for every `q`, so
a narrower band of noise does not get louder. The filter is linear and cannot
self-oscillate; `q` stops at 40.

### Saturation

`synth.drive/1` bends each channel through `tanh` with an offset:

```text
u = drive * x + bias
y = level * (tanh(u) - tanh(bias))
```

`drive` sets how hard the signal pushes into the curve, `bias` makes the
curve asymmetric so even harmonics appear, and `level` scales the result.
Subtracting `tanh(bias)` keeps silence exactly silent. A loud signal through
an asymmetric curve also shifts its average level; a `synth.highpass/1` after
the drive removes that.

The curve's added harmonics would fold back from above 24 kHz into the
audible band. The processor therefore averages the curve over each sample
step instead of sampling it: first-order antiderivative anti-aliasing (Parker,
Zavalishin and Le Bivic, DAFx-16). With `F(u) = ln cosh(u)`, evaluated as
`|u| + ln1p(exp(-2|u|)) - ln 2` so it cannot overflow,

```text
u' = drive * x_previous + bias
y  = level * ((F(u) - F(u')) / (u - u') - tanh(bias))
```

and `tanh((u + u') / 2)` replaces the quotient when `|u - u'| < 1e-6`. Both
ends of the step use the current `drive` and `bias`; the only state is each
channel's previous input, initially zero and reset like the filters. The
average delays the output by half a sample, and it needs no oversampling
filter whose design would also have to be fixed.

The averaging suppresses most strongly the harmonics that would fold down
into the low and middle band, where they sound like out-of-tune tones, and
least those that fold to just below 24 kHz. On the recorded fixture, a
4,990 Hz sine at 0.9 of full scale into `drive = 8`, folded power below 5 kHz
falls from −24.0 dB to −53.4 dB relative to the fundamental. Across the whole
band it falls only from −13.5 dB to −19.6 dB, because folds near 24 kHz
dominate the total. High notes driven hard therefore still alias near the top
of the band; `tests/drive.rs` checks both figures.

Band limiting the underlying wave does not eliminate modulation sidebands or
bank-switching artifacts. Strong/high-frequency FM can still alias; automated
spectral fixtures must characterize this residual behavior. See Nielsen's
[DAFx 2020 discussion of through-zero FM and antialiasing](https://karmafx.net/docs/karmafx_DAFx2020_paper_61.pdf).

## Wavetables

```maac
wavetable colors {
  path = "colors.wav";
  hash = "sha256:<64 lowercase hex digits>";
  cycle_length = 256;
}
```

Wavetables accept finite mono PCM or float WAV values with explicit cycle
length, a power of two from 8 through 2048 samples. A table has 1 through 32
complete contiguous frames. The WAV rate is metadata: phase, note frequency,
and cycle length determine playback. There is no inferred cycle detection,
normalization, resampling, or recorded-sample playback. Position 0 selects the
first frame and 1 selects the last; intermediate positions interpolate adjacent
frames. Phase interpolation wraps the last sample to the first. Harmonic banks
retain DC and supported harmonics without inferred normalization. Resolved raw
samples are embedded in plans; original WAV files are unnecessary to render.

## Samples

```maac
sample piano_c4 {
  path = "piano-c4.wav";
  hash = "sha256:<64 lowercase hex digits>";
  root = C4;
  loop = [12000frame, 36000frame];
}
```

A `sample` declaration pins one finite mono or stereo PCM (8/16/24/32-bit) or
float32 WAV. Unlike a wavetable, the WAV sample rate is the recorded playback
rate, and the WAV header gives the channel count. Alternatively, it pins a core
PCM file:

```maac
sample piano_c4 {
  path = "piano-c4.pcm";
  hash = "sha256:<64 lowercase hex digits>";
  format = "pcm_f32le_interleaved/1";
  rate = 48000Hz;
  frames = 2400000;
  root = C4;
}
```

With `format`, the fields `rate` and `frames` are required. The optional
`channels` is 1 (the default) or 2. The bytes must be exactly
`frames * channels * 4`, interleaved as in every core audio asset, and the
sample is carried as an ordinary plan audio asset instead of embedded values.
The execution view expands the omitted `channels = 1`. Asset-backed samples:

- follow the core audio-asset rules: 16 MiB inline in total, or with
  `--disk-media` a private disk snapshot of up to 1 GiB;
- do not count against the embedded-sample budget;
- render identically to the same values in a WAV;
- require a version 4 or later plan artifact, so the version 2 plan API refuses
  them with `E_CAPABILITY`.

Their plan asset ID is reserved as `__<sample id>`, for example `__sample_0`. A
composition asset that uses such an ID is `E_DUPLICATE_ID`.

A sample declared in a composition document may instead play one of that
document's audio assets:

```maac
asset piano_c4_pcm {
  kind = audio; path = "piano-c4.pcm"; hash = "sha256:<64 lowercase hex digits>";
  format = "pcm_f32le_interleaved/1"; rate = 48000Hz; channels = 2; frames = 2400000;
}
sample piano_c4 { asset = &piano_c4_pcm; root = C4; }
```

Its only fields are `asset`, `root` and the optional `loop`. It takes its rate,
channels, frames and provenance from the asset, and the plan carries the
recording once, under the asset's own ID, however many clips, kits and samples
play it. The reference must be a local `&id` naming an `asset` with
`kind = audio` in the same document; anything else is `E_REFERENCE`. A library
document declares no assets, so its samples cannot use this form. Such a sample
is asset-backed: it follows the rules above, including the version 4 plan
requirement and `--disk-media`.
`root` is a spelled pitch or `key(n)` with key 0 through 127. It names the key
at which the sample plays at its recorded rate. The optional `loop = [a frame,
b frame]` is a forward sustain loop with `0 <= a < b <= frames`. WAV samples
share the embedded-sample budget with wavetables and are embedded in plans with
their provenance. The budget is 262,144 values in total, so a stereo frame
counts twice. At most 64 samples may be declared.

A voice-only `synth.sample/1` node selects samples with 1 through 128 zones:

```maac
config = {
  channels = 2;
  fade_shape = equal_power;
  zones = [
    { sample = &soft_c4; low = C3; high = F4; velocity = [0, 3/5]; velocity_fade = [0, 1/5]; },
    { sample = &hard_c4; low = C3; high = F4; velocity = [2/5, 1]; velocity_fade = [1/5, 0]; key_fade = [0, 3]; },
    { sample = &hard_g4; low = D4; high = C6; key_fade = [3, 0]; }
  ];
};
```

Each zone names a `sample` export, by the same reference rules as
`config.table`, and an inclusive key range `low`..`high`. It may also give:

- `velocity = [v0, v1]`: a layer with `0 <= v0 < v1 <= 1`, covering `[v0, v1)`
  plus velocity 1 when `v1 = 1`. The default is `[0, 1]`.
- `key_fade = [l, h]`: nonnegative fade widths in semitones, measured inward
  from the low and high key edges. Their sum is at most the zone's key count.
- `velocity_fade = [l, h]`: the same for the velocity edges, with a sum of at
  most `v1 - v0`.

The node's optional `channels` is 1 (the default) or 2 and is its output
channel count. Every zone's sample must have exactly that many channels;
otherwise it is `E_PORT_TYPE`. Mono and stereo samples are never mixed, upmixed
or downmixed in one node. Like every stereo voice-graph signal, a stereo node
cannot be a modulation source.

Fades default to `[0, 0]`, which is a hard edge. Zones may be listed in any
order and may overlap. The node's optional `fade_shape` is `linear` (the
default) or `equal_power`. The editing and execution views expand all of these
defaults.

At the voice's first rendered frame, the note fixes the zones it plays. Its
frequency `f` (including any initial pitch expression, before node `ratio` and
`frequency`) gives the key position `k = 69 + 12*log2(f/440)`. Its note-on
velocity is `v`. For each axis, a zone's gain is:

- zero outside the axis interval. On the key axis that interval is
  `[low - 1/2, high + 1/2)`;
- otherwise `min(1, (x - a)/l, (b - x)/h)` over the axis interval `[a, b)`,
  where a zero width omits its term;
- mapped through `sin(pi*g/2)` when `fade_shape = equal_power`.

A zone's gain is its key gain times its velocity gain. Every zone with a
positive gain plays its own sample at that fixed gain, and the node output is
the sum in zone order. Two zones that overlap by exactly their facing fade
widths therefore crossfade, and overlapping zones without fades stack. A note
whose zones all have zero gain fails with `E_RANGE`. It is never silently
skipped or mapped to the nearest zone. Voice amplitude still multiplies by the
note velocity, as for every instrument.

Each playing zone starts at frame 0 of its sample. Each output frame reads
the core linear interpolation at the zone's current position, weights it by the
zone gain, and multiplies the sum by `level`. A stereo node does this for each
channel with the same position and gain, so each output channel equals a mono
node playing that channel of every sample. Each zone then advances its
position by `(f*ratio + frequency) / root_hz * wav_rate / 48000`, using its own
sample's root and recorded rate. Here `f` includes the current pitch expression, so bends
stay continuous, and `root_hz = 440*2^((root-69)/12)`. The playback frequency
must be positive. Inside a loop, reaching `b` wraps to `a + (position - a) mod
(b - a)`, and the right neighbour of frame `b-1` is frame `a`, so the loop keeps
sounding through release. Without a loop, values past the recording are zero.
The voice still lives until its amplitude envelope finishes. No crossfade,
layer, normalization, or resampling filter is inferred beyond the declared
zones. See the
[sample vectors](../tests/sample_instrument.rs).

## Rust and interchange boundaries

`SourceBundle` owns project-relative source text and asset bytes. `check_bundle`
validates either a composition or all library exports; `compile_bundle`
requires a composition. Filesystem access belongs to a separate loader and the
CLI, never the compiler or renderer. Existing parsed-document `check` and
`compile` APIs remain; documents with any imports, including built-ins, fail explicitly
as unresolved. Built-in-only compositions use `SourceBundle::new` with just
the entry text and compile through `compile_bundle`.

Version 2 performance plans carry reusable graph programs, embedded wavetable
data, source/dependency identities, entry-file identity, and graph source
provenance, including built-in source hashes and resolved noise seeds. Node
instances reference programs and carry resolved public control values. Loading and rendering an untrusted plan independently validates graph
structure, types, references, controls, timing, and resource limits. Version 1
plans remain accepted; legacy-only single-document compilation may retain
version 1. Version 1 must reject version 2 payloads instead of ignoring them.

Limits apply together: 4 MiB per source/asset input, 16 MiB aggregate sources,
16 MiB aggregate assets, 64 source files, 64 assets, import depth 32, and 200,000
aggregate syntax objects. Graph limits additionally bound programs, nodes,
connections, modulation, controls, embedded samples, allocated voice state,
and sample execution work. The implementation must publish the exact numeric
graph limits and enforce them before allocation or rendering. Existing exact
arithmetic, event, output duration, and mono/stereo limits remain applicable.

Hosted discovery, registries, executable plugins, GUI editing, real-time audio,
and feedback are outside this implementation. Recorded-sample instruments are
the [samples](#samples) above. Listening evidence is recorded separately from
tests; the only record so far is the informal
[showcase approval](showcase-listening.md).
