# Plucked-string extension: implementation contract

**Status: implemented; recorded kernel, instrument, and installed-authoring
checks passed on the tested host. User listening acceptance remains pending.**
The [delivery report](acoustic-delivery.md) distinguishes representative
signal-pitch measurements from full-range phase/range checks and records the
observed evidence. The approved contract retains the review corrections about
damping and node-local error behavior; no realism approval is implied.

This extension lets implementers and instrument authors specify a recirculating
string with changing partial decay. It adds one mono voice-only processor,
`synth.pluck/1`, and a separate `std/acoustic/1.0.0` library. Existing processors,
graph DAG rules, scheduling, ADSRs, plan versions, and the released bytes and
default discovery behavior of `std/basic/1.0.0` remain unchanged.

It models a string, not recorded audio or a complete guitar body. Pluck
position, excitation hardness, a new body resonator, and velocity-dependent
timbre are excluded. Existing filters may provide body weighting. Improved
acoustic realism requires the user's listening assessment; numerical success
does not establish it.

## Source and parameter contract

The processor has one mono `out`, no audio input, and no separate event port.
It is legal only in a voice graph, which still requires a designated amplitude
ADSR. Source configuration is optional: omission and `config = {};` resolve
to seed 1831565813. A nonempty configuration contains exactly `seed`, a
dimensionless integer in 1…4294967295. Seed is immutable configuration, never a
control or automation target. Unknown configuration fields fail.

| Parameter | Unit | Default | Inclusive range | Rate |
| --- | --- | --- | --- | --- |
| `ratio` | Dimensionless | 1 | 1/8…8 | Sample |
| `decay` | Seconds | 3 s | 1/20…30 s | Sample |
| `damping` | Dimensionless | 1/2 | 0…1 | Sample |
| `level` | Dimensionless | 1 | 0…16 | Sample |

Existing rational unit/range validation, public controls, and mono sample-rate
modulation apply to all four parameters. There is no oscillator `phase` or
frequency-offset parameter. At each frame, after controls and modulation,
`f = expressed_base_hz * ratio` must be finite and in 20…4000 Hz inclusive. This
effective-frequency constraint applies even when level, velocity, or amplitude
is zero. A valid ratio alone does not guarantee a valid effective frequency.
General live expressions are checked at render time without clipping,
absolute-value correction, or fallback.

`decay` describes the nominal 60 dB amplitude-reduction time from uniform loop
loss alone at fixed pitch. Interpolation and damping add loss; total measured
T60 is not guaranteed. Time-varying parameters can change observed decay.
`damping = 0` removes the additional two-tap loss filter; `damping = 1` gives
that filter its strongest high-frequency attenuation. Because interpolation
is retuned when damping changes, combined-loop attenuation and decay need not
vary monotonically with damping. Damping, brightness, and decay interact.

This standalone library example separates natural string decay from ADSR release:

```maac
maac 1;
library string_example { version = "1.0.0"; }
instrument string {
  channels = 1;
  voice voice {
    channels = 1;
    amplitude = &amp;
    output = &string:out;
    node amp {
      type = "synth.adsr/1";
      params = { attack = 2ms; sustain = 1; release = 300ms; };
    }
    node string {
      type = "synth.pluck/1";
      config = { seed = 1831565813; };
      params = { ratio = 1; decay = 3s; damping = 0.5; level = 0.12; };
    }
  }
  control release { target = &voice.amp.params.release; default = 300ms; }
  control bend_ratio { target = &voice.string.params.ratio; default = 1; }
}
```

## Exact initialization and recurrence

The reference sample rate is 48,000 Hz. Each pluck node per live voice owns
exactly `C = 2402` f64 ring cells `B`, write index `w`, and previous delayed
read `p`. At note-on initialize a u32 state from the resolved seed and fill
`B[0]` through `B[2401]` in ascending order, advancing before each stored sample:

```text
state ^= state << 13
state ^= state >> 17
state ^= state << 5
B[index] = state / 2147483648.0 - 1.0
```

Left shifts discard bits beyond 32 bits; right shifts are logical. Set `w=0`
and `p=0`. There is no system entropy, event-address hashing, normalization,
mean subtraction, or extra excitation filtering. Initialize the entire ring,
including cells outside the initial delay span. Rendering introduces no further
random values. The ring is excitation history, so the first frame reads it
immediately without an extra period of silence. Same-seed voices start identical
state but own independent storage; pitch determines the first cells read.

First evaluate and validate the current parameters and effective frequency.
Then calculate in binary64 in this order, using `f64::consts::PI` and ordinary
non-fused arithmetic without an explicit `mul_add`:

```text
omega = (2 * PI * f) / 48000
b = damping / 2
a = 1 - b
phi = atan2(b * sin(omega), a + b * cos(omega))
D = 48000 / f - phi / omega
N = floor(D)
alpha = D - N
theta = alpha * omega
mu = 0, if alpha == 0
     sin(theta) / (sin(omega - theta) + sin(theta)), otherwise
g = pow(10, -3 / (f * decay))
```

Before updating the node's string state, verify finite coefficients,
`11 <= N <= 2400`, `0 <= mu <= 1`, and `0 < g < 1`. Convert `N` to an integer
only after checking its range. Unexpected numerical states fail without
clamping. Then compute:

```text
i0 = (w + C - N) % C
i1 = (w + C - N - 1) % C
v = (1 - mu) * B[i0] + mu * B[i1]
z = a * v + b * p
B[w] = g * z
p = v
w = (w + 1) % C
output = v * level
```

Read both old ring values before writing. Do not add dry excitation to the
feedback write. Output is `v * level`, not the filtered value or new write.
Level affects only output. The same loop history advances while output is muted.

The convex interpolator's phase delay at `omega` is
`atan2(mu*sin(omega), 1-mu+mu*cos(omega))`. The formula for `mu` solves for
phase `theta`, giving `N*omega + phi + theta = 2*PI` on the unit circle at
fixed parameters. This is not an exact damped-pole or harmonic-tuning claim.
Convex interpolation and loss-filter weights with `0<g<1` bound internal state
by the initial maximum, subject to floating-point tolerance, including under
valid parameter changes. Output gain and summing still require export headroom.

For clarity, combined-loop high-frequency loss is not the monotonic oracle:
at `f=48000/100.5`, the tenth-partial interpolation-plus-loss-filter attenuation
is approximately −0.431474, −0.639149, and −0.431474 dB for damping 0, 0.5,
and 1 respectively. Only the separate two-tap loss filter has the required
monotonic attenuation property.

Coefficient caching is permitted only for identical evaluated
`(f, decay, damping)` values with unchanged reference samples. Resource
accounting assumes coefficients may change every sample. Per-sample allocation
is prohibited.

## Automation, lifecycle, and reset

Sample-rate changes take effect on the current frame. Ratio, decay, and damping
changes never refill the ring or reset `w` or `p`, including integer-delay
crossings. There is no implicit smoothing, crossfade, slew limit, or retrigger.
Smooth bend and vibrato curves retain string history. Abrupt valid changes may
produce artifacts; stability does not promise click-free arbitrary automation.

Existing global control automation affects all active voices in an instrument
instance. [Per-note gain](instrument-gain.md) applies after the full voice
graph, ADSR, and velocity, before voice summation and shared effects. Zero gain
never retriggers or refills the string; its recurrence continues normally.
[Per-note pitch](instrument-pitch.md) supplies
`expressed_base_hz = note_pitch_hz * 2^(cents / 1200)` independently for each
voice, with gate-end holding through release. The base remains finite, positive,
and strictly below Nyquist; the effective pluck frequency remains 20…4000 Hz
inclusive after the live ratio. There is no frequency offset. Bends preserve
ring pointers and history without refilling or retriggering. Existing mono
graph modulation can drive ratio, including an LFO.

Zero level, zero velocity, and amplitude-envelope silence still advance the
entire recurrence. The designated ADSR and velocity multiply voice output
exactly once. Note-off changes the ADSR release, not string state. Natural
decay, silence, and zero sustain do not retire a voice before its designated
release reaches zero. Existing note-off/retirement/note-on ordering and
project-end release and tail boundaries remain authoritative.

Reset discards voices and rings. The same subsequent seed, note, and parameter
history reproduce samples in the same executable/environment. Shared effects
retain existing reset/tail behavior. Cross-platform bit identity is not claimed.

## Strict plan, errors, and compatibility

Version 2 instrument graph nodes gain this exact processor object:

```json
{"kind":"synth.pluck/1","seed":1831565813}
```

The seed is always resolved and present. Missing, zero, negative, fractional,
or overflowing seeds, unknown fields (including `channels`), and unsupported
versions fail loading. Parameters remain in the existing canonical-rational
node `params` map, not the processor object. Typed in-memory plan validation
enforces the same contract. Mutable ring, indices, and PRNG state are not
serialized. Version 1 continues to reject instrument payloads; existing
version 2 plans remain valid.

Retained plans render independently of sources and the current library
registry. Source/dependency hashes are internally validated provenance, not
registry authentication of an edited plan.

| Invalid condition | Behavior |
| --- | --- |
| Static parameter outside its interval | Existing `E_RANGE` diagnostics |
| Wrong units, unknown source fields/parameters, invalid seed syntax | Existing strict source diagnostics; no coercion |
| Missing/unknown wire fields or invalid wire seed | Strict plan-load error |
| Shared-stage pluck | `E_CAPABILITY` |
| Audio input or invalid connection | Existing graph/reference/channel error |
| Audio, modulation, or mixed-edge cycle | Existing cycle error; no feedback-edge exception |
| Invalid evaluated frequency, coefficient, or nonfinite DSP value | `E_NONFINITE`; frequency/coefficient checks precede that pluck node's state update |
| Memory/work limit, new pluck-storage allocation failure, arithmetic overflow | `E_RESOURCE_LIMIT` before rendering or affected allocation |

Error atomicity is node-local. Earlier nodes or voices may already have advanced
before a later node fails; this extension does not promise graph/frame rollback.
For bounded allocation/resource failures, use existing `RenderError::Plan`
carrying `E_RESOURCE_LIMIT`, without adding a public error-enum variant.
New pluck ring storage and new pluck voice-capacity reservations MUST use
fallible allocation and return `E_RESOURCE_LIMIT` on failure, without panic or
false success. Existing graph containers and string allocations retain their
existing behavior. This extension does not promise global out-of-memory
recovery or require a legacy allocator refactor. Existing output file
failure-preservation behavior applies.

## Memory and work accounting

All existing graph, voice, source, plan, and work limits continue to apply.
The additional ceiling is **8,388,608 pluck delay cells**, or **64 MiB f64
payload**, per complete plan or independently constructed direct runtime.
Each pluck voice-node costs 2402 cells / 19,216 payload bytes plus bounded
scalar/container overhead. The limit admits at most 3492 such allocations.
Rings use separate fallible heap storage; a 2402-element inline enum member
must not enlarge every existing processor-state variant.

Compute the following with checked arithmetic over instrument instances:

```text
pluck_cells = sum(declared_voice_capacity * voice_graph_pluck_node_count * 2402)
```

Use declared capacity, not expected overlap or audibility. Unconnected pluck
nodes in instantiated graphs count. Reject excess before constructing engine
runtimes or allocating per-voice rings. Caller plan limits may tighten the
ceiling. Compiling an instrument records counts without per-voice ring allocation.

Expose the tighter caller budget as `PlanLimits::max_pluck_delay_cells`, with
the default and enforced maximum above. This additive public Rust field requires
downstream exhaustive `PlanLimits` struct literals to add the field or use
`..PlanLimits::default()`. Default constructors and retained JSON plans remain
compatible; `PlanLimits` is not serialized into performance plans.

Direct `InstrumentRuntime::new` independently enforces the same ceiling for its
requested capacity and compiled pluck count. Lazy allocation at `note_on` cannot
bypass preflight; ring allocation remains fallible. Hosts are responsible for
aggregate memory across independently owned direct runtimes; complete engine
plans are centrally aggregate-checked.

Under the default 500,000,000 execution-work allowance, charge **16 units
per pluck node visit**, keeping old node and edge/modulation weights unchanged,
plus **2402 initialization units per actual note per pluck node**. Count muted
and zero-velocity notes. Sample work uses the existing gate-plus-maximum-release
span clipped to the render endpoint, without discounting cached coefficients.
Reject excess initialization/sample work and overflow before rendering.

The [explicit song execution profile](project-entrypoint.md#explicit-finite-song-profile)
permits up to 10,000,000,000 work units by caller selection. It changes neither
these weights nor pluck memory and other resource limits; source/plan data
cannot select the larger allowance. Its integrated and native-song validation
is recorded in the [entrypoint delivery report](project-entrypoint-delivery.md),
separately from this processor's numerical evidence.

Open-ended direct runtimes enforce memory/capacity and fallible allocation for
the new pluck storage and voice-capacity reservations;
their host bounds render calls. This extension adds no direct total-work budget
API. A benchmark must exercise per-sample coefficient changes. Normalized work
units do not guarantee wall-clock execution time.

## Acoustic library and discovery

The separate self-contained library `std/acoustic/1.0.0` exports exactly
`nylon_guitar`, `steel_guitar`, and `muted_guitar`, using this string processor
and existing graph components. It adds no samples, network access, executable
plugins, or dependency. Existing basic-library source bytes remain frozen.

Each export has stereo output and common public controls: `level` scales all
layers; `brightness` is a one-pole cutoff in Hz; `release` is designated ADSR
release in seconds; `pan` is equal-power stereo position. Dimensionless string
damping must not replace Hz-valued brightness. Initial expressive controls are
`bend_ratio` and `vibrato_amount`; their exact targets/defaults/ranges are
specified with the reviewed library graphs before source freezing. Additional
public decay/damping controls are outside this initial requirement.

Selection is exact:

```maac
import acoustic { builtin = "std/acoustic/1.0.0"; }
```

The reserved identity is `@builtin/std/acoustic/1.0.0.maac`. Existing reserved
namespace protection, hash provenance, deduplication, resource accounting, and
no-fallback behavior apply. Basic and acoustic may coexist under explicit aliases.

Additive Rust discovery has these signatures:

```rust
pub struct LibraryInfo {
    pub library: String,
    pub source_path: String,
    pub source_hash: String,
}
pub fn libraries() -> Vec<LibraryInfo>;
pub fn catalog_for(library: &str) -> Result<Catalog, Diagnostics>;
pub fn instrument_in(library: &str, name: &str) -> Result<InstrumentInfo, Diagnostics>;
```

`LibraryInfo` identifies the exact version, reserved source path, and hash of
the embedded source bytes. `Catalog` and `InstrumentInfo` retain their existing
shapes. `catalog()` and `instrument(name)` remain wrappers for the basic
library, including existing default results. Unknown exact library identities
and instrument names fail explicitly through diagnostics; there is no fallback.
Existing `lookup(id) -> Option<BuiltinSource>` retains its signature.

CLI adds `instruments --library ID [NAME]` and `instruments --libraries`.
`--libraries` conflicts with both a positional name and `--library`. Unknown
exact IDs produce `E_REFERENCE`. No latest-version alias is added. Explicit
library selection adds an optional top-level JSON `library` string identifying
the selected version. `--libraries` returns an optional top-level `libraries`
array of `LibraryInfo` records with `command` equal to `instruments` and `input`
equal to `@builtin`. Selected-library listing uses the existing `catalog`
payload; named detail uses `instrument`. Omitting a selector defaults to
`std/basic/1.0.0`. Unused optional fields are omitted; existing default CLI
JSON fields and behavior remain unchanged.

## Required numerical and listening evidence

These remain the acceptance requirements. Observed results and their scope are
recorded separately in the [delivery report](acoustic-delivery.md):

1. Independent synthetic-ring oracles must verify integer/fractional read
   indices, read-before-write, immediate output, and scalar-history updates.
   Expected arithmetic must not call the production kernel.
2. Independently verify seed 1, default-seed, and max-u32 initialization.
   With seed 1, `f=480`, `damping=0`, `decay=3`, and `level=1`, `N=100`,
   `mu=0`, and the first six outputs are `0.9930807766504586`,
   `0.26170715782791376`, `-0.9504082570783794`, `0.9612405328080058`,
   `-0.7509497245773673`, and `-0.03659906983375549` (cells 2302…2307).
   Frame 100 is `g*output[0]`, with `g` approximately `0.9952144352021834`.
   Binary arithmetic vectors compare exactly on one host; transcendental
   comparisons state their tolerance.
3. Independently check wrapped phase error below `1e-12` radians across
   E2…E6 semitones, ±2-semitone bends, effective-frequency endpoints 20/4000 Hz,
   and damping 0/0.5/1. Also measure signal-derived pitch at representative
   default low/mid/high notes with a documented independent estimator and
   sufficient post-excitation duration. Target: at most 2 cents over E2…E6;
   formula agreement alone is insufficient. Any adjustment needs reviewed
   specification evidence.
4. Verify increasing high-frequency attenuation for the separate two-tap
   loss filter at fixed frequency. Do not impose that monotonic property on
   the retuned combined loop or arbitrary samples. Measure early/late spectral
   bands to establish natural upper-partial decay.
5. Stress changing ratio/decay/damping at valid boundaries: finite internal
   state, magnitude at most `1+1e-12`, and no reinitialization at integer-delay
   crossings. Separately exercise smooth ±2-semitone bends and 5 Hz vibrato.
6. Verify voice/instance isolation, seed replay, zero-level/velocity/envelope
   advancement, ADSR/velocity applied once, note-off retirement, reset, and
   exact same-host source/plan roundtrip samples. Preserve legacy processor
   regression evidence and frozen basic hashes.
7. Reject bad seed, units/ranges, unknown fields, shared use, audio inputs,
   cycles, and invalid effective frequency without updating the affected
   pluck node. Cover source, wire, typed-plan, and direct-runtime boundaries.
8. Test maximum accepted cell/capacity budgets, a tightened limit one cell
   below required storage, first excessive capacity, overflow, initialization
   work, sample work, and new pluck storage/reservation allocation failure. Check direct APIs and no
   per-sample allocation. Benchmark worst-case changing coefficients.
9. Validate all acoustic exports/controls, selected-library discovery and old
   defaults, mixed/repeated imports, unknown versions, retained-plan rendering,
   deterministic finite output, PCM16 headroom, and release silence.
10. Provide level-matched old/new guitar auditions with identical musical
    material, velocity, format, and documented matching gains. Include
    low/mid/high notes, chords, short/long gates, bends, and vibrato. Record
    artifacts and the user's assessment of attack, string decay, timbre,
    tuning transitions, artifacts, and overall acoustic-guitar improvement.
    Without user listening, mark perceptual acceptance pending; no realism
    or listening-approval claim follows from numerical checks.

## Design lineage

Recirculating excitation and loop filtering follow the string-synthesis family
described by [Karplus and Strong (1983)](https://www.moforte.com/wp-content/uploads/2020/05/Karplus-Strong-CMJ-1983.pdf).
The tuning and decay issues are discussed by
[Jaffe and Smith (1983)](https://musicweb.ucsd.edu/~trsmyth/papers/KSExtensions.pdf).
The convex interpolation derivation, seed mapping, numeric ranges, and resource
policy above are this proposal's choices, not claims of reproducing those
papers' complete algorithms. This contract does not use an allpass interpolator
or increase loop gain to compensate for extra loss.
