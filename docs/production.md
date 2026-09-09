# Native mixing and production deliveries

This is the normative contract for the required capability `maac.production/1`.
An **experimental Rust implementation** is available through native nodes,
retained plans, and `maac deliver`. The true-peak profile uses one Annex 2
four-times interpolator; the [metering evidence](production-metering-evidence.md)
records external acceptance results and the superseded two-stage profile.
Syntax, schema, and bounded arithmetic
fixtures provide specification evidence only; they do not establish native
rendering, SRC, or metering conformance.

The capability adds project-level native `fx.eq/1`, `fx.compressor/1`, and
`fx.reverb/1` nodes and named master/stem deliveries. It leaves `maac 1`, Core
Audio conformance obligations, the grammar, the generic syntax-tree schema, and
implemented performance-plan versions unchanged. Expressive instruments,
multisampling, reusable sections, and take editing remain separate future work.

## 1. Capability, source, and identity

A project using any feature here MUST include `"maac.production/1"` in
`project.requires`. Native processors use existing `node` syntax and require no
external executable asset. They forbid `implementation` and `state`. Their
reset contracts below are mandatory; an implementation cannot substitute a
similarly named plug-in. Unsupported required capabilities fail before rendering.

Named deliveries use exactly one `extension` with
`namespace = "maac.production/1"`, `render_affecting = true`, and a `schema`
reference to a descriptor asset containing the self-contained
[`production.schema.json`](../production.schema.json), pinned by its exact byte
SHA-256. The schema validates the tagged canonical syntax-tree representation
of this extension's `data`, not the entire document. Schema validation does not
replace reference resolution, exact unit conversion, or the semantic checks in
this contract. Native processing without named deliveries does not require an
otherwise empty extension object. Delivery definitions require at least one
entry in `data.deliveries`.

Delivery and target IDs match `[A-Za-z_][A-Za-z0-9_]*` and contain at most
128 ASCII characters. Each delivery has exactly `rate`, `resampler`, and `targets`. `rate` is an exact
frequency equal to 44100, 48000, or 96000 Hz; `resampler` is the string
`"maac.src.kaiser/1"`. Each nonempty `targets` record maps stable IDs to records
with required `role`, `output`, `encoding`, and `dither`, and optional `limits`.
`role` is the symbol `master` or `stem`; `output` is a resolved audio-output-port
reference. `encoding` is `wav_f32le`, `wav_pcm24le`, or `wav_pcm16le`.

There MUST be exactly one master per delivery, and its port MUST equal
`project.output`. All other targets are stems. Infer one or two channels from
the referenced port; reject unsupported layouts and non-audio ports. There is
no target channel-count override, channel conversion, source filename, or path.
Caller-controlled destination mapping maps the selected delivery and target IDs
to files. Unknown fields, unknown enum values, dangling references, duplicate
IDs, and unsupported units are errors. Definitions may share a port or use
different formats and rates for separate releases.

`dither` is explicitly either `{ type = none; }` or
`{ type = tpdf; seed = 123; }`, where `seed` is an exact unsigned 64-bit integer.
Float32 requires `none`; integer PCM permits either form. No implicit project
seed or default dither policy applies.

The complete extension participates in the existing execution hash, including
limits and unselected delivery definitions. It MUST NOT be treated as an inert
analysis annotation. The delivery/render identity additionally includes the
selected delivery and target IDs in unsigned UTF-8 order, resolved ports and
channel order, output rate, exact interval, encoding, resampler identity and
constants, coefficient/arithmetic implementation, dither identity and seed, and
analyzer/profile identity and numerical environment, together with the existing
render-key dependencies. Analyzers do not feed back into graph execution.

## 2. Shared native processor rules

The initial profile requires `project.rate = 48000Hz`. Delivery conversion does
not change the engine rate. Every processor requires `config.channels` equal to
1 or 2, with a single-connection audio input `in` and audio output `out` of that
width. Main input connections are required; disconnected input is not an
implicit zero. The compressor may additionally have the sidechain below.

Only the config and parameter fields defined below are accepted. Config is
immutable and cannot be an automation or modulation target. Continuous params
are evaluated at every engine sample after core automation/modulation rules,
including tail freezing of score automation. Their range policy is `error`:
validate base values, automation values, and the effective modulated value.
All arithmetic/state is finite binary64; reject nonfinite input, parameter,
coefficient, intermediate result, or committed state. There is no silent
clamping, denormal suppression, normalization, limiting, smoothing, or recovery
by resetting damaged state. A locked byte-identity claim additionally pins the
math implementation, floating-point environment, and fused-operation policy.

State resets once at the project's reset origin, persists through parameter
changes and the entire declared tail, and is private to each node instance.
Technical latency is zero for all three processors. They conservatively declare
current-sample feedthrough from every main input channel to its corresponding
output and from all effective parameters to affected outputs. EQ has no
cross-channel history; compression has linked-channel dependencies. Reverb's
dry path gives feedthrough even when a particular mix value mutes that path.
These declarations do not dynamically change to break graph cycles.

## 3. EQ — `fx.eq/1`

Each node implements one biquad band. Use explicit cascades for multiple bands.
Required `config.mode` is one of `peak`, `low_shelf`, `high_shelf`, `low_pass`, or
`high_pass`; `channels` and `mode` are its only config fields.

| Parameter | Applicable modes | Default | Allowed range |
| --- | --- | --- | --- |
| `frequency` | All | `1000Hz` | Strictly greater than 0 and less than 24000 Hz |
| `q` | Peak, low-pass, high-pass | `1` | Dimensionless 0.1 through 18 |
| `gain` | Peak, both shelves | `0dB` | −24 through +24 dB |

Explicitly specifying `q` for a shelf or `gain` for a pass filter is an error;
no ignored parameter is accepted. Defaults exist only for applicable fields.
Shelves fix slope `S = 1` and do not expose bandwidth or slope controls.

Use the peaking-EQ, low-shelf, high-shelf, low-pass, and high-pass equations of
the pinned [W3C Audio EQ Cookbook, 8 June 2021](https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/).
Let `w0 = 2*pi*frequency/48000`, `A = 10^(gain/40)` for modes with gain. Peak
and pass filters use `alpha = sin(w0)/(2*q)`. Shelves use the Cookbook shelf
alpha expression with `S = 1`; they do not substitute Q. Divide all numerator
and denominator coefficients by the Cookbook `a0`, obtaining `b0,b1,b2,a1,a2`
with normalized `a0 = 1`.

For each channel, direct form I is:

```
y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2]
       - a1*y[n-1] - a2*y[n-2]
```

Evaluate terms in displayed order. All four histories per channel reset to
zero. Compute coefficients from the current sample's parameters before that
sample's output, then shift the histories. Parameter changes retain histories;
there is no coefficient interpolation or additional smoothing. Reject nonfinite
coefficients/results rather than falling back to bypass. For gain zero, the
mathematical peak/shelf response is unity; that is not permission to reset or
skip state updates.

## 4. Compressor — `fx.compressor/1`

`config.detector` defaults to `internal` and is immutable. In `external` mode,
required `config.sidechain_channels` is 1 or 2 and declares an audio input named
`sidechain` accepting exactly one explicit connection of that width. Internal
mode forbids `sidechain_channels` and has no sidechain port. No implicit bus,
self-detection fallback, or channel conversion applies.

| Parameter | Default | Allowed range |
| --- | --- | --- |
| `threshold` | `-18dB` | −120 through +24 dB |
| `ratio` | `4` | Dimensionless 1 through 100 |
| `knee` | `6dB` | 0 through 24 dB |
| `attack` | `10ms` | 0 through 10 s |
| `release` | `100ms` | 0 through 30 s |
| `makeup` | `0dB` | −24 through +24 dB |

The feedforward sample-peak detector is `p = max_c(abs(detector_input[c]))`,
using the main channels in internal mode and the sidechain channels in external
mode. External detector audio never enters the audible sum. Outputs depend on
all detector channels at the current sample in addition to their main channel.
No RMS detector, lookahead, automatic makeup, or detector filtering applies.

For positive `p`, let `L = 20*log10(p)`, `v = L-threshold`, `R = ratio`, and
`W = knee`. Desired nonnegative gain reduction `G` in dB is:

```
W == 0:  G = max(0, (1 - 1/R)*v)
W > 0:
  v <= -W/2: G = 0
  v >=  W/2: G = (1 - 1/R)*v
  otherwise: G = (1 - 1/R)*(v + W/2)^2/(2*W)
```

For `p = 0`, define `G = 0` directly without evaluating log zero. This is the
hard/soft static knee expressed as positive reduction, followed by first-order
smoothing in the logarithmic gain domain, consistent with
[Giannoulis, Massberg and Reiss (2012)](https://joshreiss.github.io/documents/2012/GiannoulisMassbergReiss-dynamicrangecompression-JAES2012.pdf).
Let previous smoothed reduction be `z`, initially zero. Choose `tau = attack`
when `G > z`, otherwise `tau = release`. For `tau = 0`, set `z_new = G`;
otherwise `a = exp(-1/(48000*tau))` and
`z_new = a*z + (1-a)*G`. Time constants are e-folding times, not the time to
reach a percentage assigned by a user-interface convention. Apply
`10^((makeup-z_new)/20)` to every main channel, then retain `z_new`. Automation
changes the current target and coefficient without resetting reduction.

## 5. Reverb — `fx.reverb/1`

This is an original eight-delay feedback network, not an emulation or an
acoustic-realism claim. Its only config fields are `channels`, `predelay`, and
`damping`.

| Field | Evaluation | Default | Allowed range |
| --- | --- | --- | --- |
| `config.predelay` | Immutable | `0s` | 0 through 250 ms |
| `config.damping` | Immutable | `0.5` | Dimensionless 0 through 1 |
| `params.decay` | Every sample | `1.5s` | 0.1 through 30 s |
| `params.mix` | Every sample | `0.2` | Dimensionless 0 through 1 |

Indices in this section start at zero. Construct the Sylvester matrices by
`S1 = [1]`, `S(2m) = [[Sm,Sm],[Sm,-Sm]]`, and set `H = S8/sqrt(8)`. The input
projection matrix `B` consists of H's first `channels` columns; wet projection
is `transpose(B)` with no additional normalization.

Each input first passes through a predelay of
`P = ceil(48000*predelay_seconds)` frames; P zero passes the current input
exactly. This creative predelay is not compensated. It next passes through two
serial Schroeder allpasses of 149 and 211 frames, both with coefficient
`a = 1/2`. For each allpass, read the old ring cell `b`, compute
`y = b-a*x`, and write `x+a*y` to that cell, advancing its index modulo length.
Thus zero-state direct feedthrough through both allpasses is `1/4`.

The eight FDN ring lengths, in order, are:

```
1493, 1601, 1747, 1867, 1999, 2137, 2281, 2437
```

At each sample, let `u` be the preprocessed input vector. Read all old FDN
ring cells into `r[0..7]` before any FDN write. With `d = damping`, calculate:

```
f[i] = (1-d/2)*r[i] + (d/2)*previous_r[i]
g[i] = 10^(-3*(length[i]/48000)/decay)
wet[c] = sum(i=0..7, B[i,c]*r[i])
write[i] = sum(c=0..channels-1, B[i,c]*u[c])
           + g[i]*sum(j=0..7, H[i,j]*f[j])
out[c] = (1-mix)*input[c] + mix*wet[c]
```

Sum in ascending indicated index order. Feedback gains belong to destination
rings. After all writes have been computed and checked finite, commit the FDN
writes, advance all ring indices modulo their lengths, and set `previous_r = r`.
Damping acts only inside feedback; wet output uses the undamped old ring reads.
Every predelay/allpass/FDN cell, index, and `previous_r` resets to zero.

The network continues running and receiving input at `mix = 0`; dry or wet
muting never discards its state. All state and delay processing continues
through the declared tail. `decay` is a nominal delay-dependent feedback decay
constant; the damping, network modes, and input spectrum affect measured decay.
There is no inferred tail duration.

Logical storage is `15562 + channels*(P+360) + 8` binary64 history values,
plus ring indices and bounded coefficient/scratch storage. Account for this
before allocation, using checked integer arithmetic, together with existing
project resource budgets. A renderer MUST impose and report finite aggregate
memory/work limits and fail before exceeding them. Per sample, this reference
uses eight damping updates, eight gains, an 8 by 8 feedback matrix product,
two 8 by channels projections, and two allpasses per channel; there is no
iteration to convergence or unbounded work. An optimized Hadamard transform
needs numerical-equivalence evidence for its changed accumulation order.
Check all intermediate/state values for finiteness before committing a frame;
finite outputs alone do not prove finite internal feedback state.

A mono unit impulse with zero predelay and `mix = 1` produces zero wet output
through frame 1492 and `1/32` at frame 1493 in the real-valued reference:
allpass feedthrough `1/4` times input/output projection product `1/8`.
Numerical verification and subsequent listening acceptance are separate gates.

## 6. Shared execution, timing, and selection

All targets in a delivery observe the same complete graph execution. Selecting
a stem MUST NOT mute other tracks or remove sidechains, shared effects,
automation, nonlinear inputs, or other processing context. A target captures
exactly its selected port. Stems need not reconstruct the master: dry taps,
shared sends, nonlinear sums, and master processing make that a separate
artistic/graph property. Selecting fewer targets may avoid storing unselected
ports, but cannot change the executed context or reset schedule.

Let `D = T(score.end)-T(score.start)+tail` be the exact physical project duration
in seconds, including all exact tempo integration. Engine frames number
`ceil(48000*D)` and delivery frames number `ceil(delivery_rate*D)`. Compute the
latter directly from exact D, never by rescaling a rounded engine frame count.
Target frame zero represents the reset origin. All targets use the same full
interval and declared tail; there is no target-specific crop, reset, inferred
tail, silence trimming, automatic fade, or gain correction.

## 7. Resampling — `maac.src.kaiser/1`

This identity specifies a centered rational-polyphase Kaiser-windowed sinc
converter from the 48000 Hz engine stream to a supported delivery rate. At
48000 Hz it MUST be exact sample passthrough, with no filter arithmetic or
added delay. For another rate, reduce `delivery_rate/48000` to coprime positive
integers `U/Ds`. Use the expanded-grid rate `48000*U`, half-length
`K = 256*max(U,Ds)`, Kaiser beta `14`, and normalized cutoff
`fc = 0.48/max(U,Ds)` cycles per expanded-grid sample (0.96 times the lower
Nyquist). The delay variable Ds here is unrelated to project duration D.

Define the real-valued prototype for integer q:

```
sinc(v) = sin(pi*v)/(pi*v), with sinc(0) = 1
I0(z) = sum(k=0..infinity, (z*z/4)^k/(k!)^2)
w(q) = I0(14*sqrt(1-(q/K)^2))/I0(14)
p(q) = 2*fc*sinc(2*fc*q)*w(q)     when abs(q) <= K
p(q) = 0                        otherwise
```

For every phase `r` from 0 through U−1, normalize its real coefficients by
`Z[r] = sum(p(q) for -K <= q <= K and q mod U = r)`. Each stored coefficient
`h(q) = p(q)/Z[q mod U]` is the correctly rounded binary64 value of that
real expression, using round-to-nearest ties-to-even. This is per-phase DC
normalization, not a second global gain adjustment; do not normalize rounded
coefficients again. The numerical implementation MUST provide sufficiently
accurate coefficient construction to establish this rounding, or distribute a
verified coefficient table and record its digest. Pin coefficient-generation
implementation and table identity in the manifest.

For output frame n and each channel:

```
y[n] = sum(k in ascending integer order,
           x[k] * h(n*Ds-k*U))
```

Include exactly those k for which `abs(n*Ds-k*U) <= K`. Read x as zero outside
`0 <= k < ceil(48000*D)`. Multiply then accumulate from positive zero in
binary64 round-to-nearest ties-to-even, without fused multiply-add,
reassociation, reduced precision, or implicit denormal flushing. Phase is exact:
output n is centered on input position `n*Ds/U`, so output zero aligns with
input zero. The centered access compensates the FIR delay exactly; do not append
filter latency or flush frames to the delivery length. Zero extension supplies
all boundary samples, including the end; edge behavior is part of this profile.
Reject any nonfinite conversion result.

The finite filter requires no convergence loop while rendering. Preflight
coefficient allocation and work with checked sizes and caller resource limits;
coefficient generation is outside sample execution. Each output's support is
bounded by `floor(2*K/U)+2` input positions per channel. Required independent
SRC tests include DC phase gain, impulse phase/alignment, edge zero extension,
passband/stopband response, alias rejection, and all three rate pairs, in
addition to exact frame-count tests.

The planning-stage in-memory exploration reportedly found sampled passband and
stopband errors below `10^-6` for these constants. This is historical feasibility
evidence, not a reproduced check in this revision, a certified continuous-
frequency bound, or renderer conformance.

## 8. Encoding and deterministic dither

Write little-endian RIFF/WAVE with mono or stereo ordering inherited from the
port. `wav_f32le` uses IEEE Float32 format; `wav_pcm16le` and `wav_pcm24le` use
signed PCM with respectively 16 and 24 bits. Encode interleaved frames, channels
in declared order. Reject unsupported container sizes before publication; this
profile does not silently switch to RF64 or another container.

Float32 rounds finite binary64 samples to IEEE binary32 using nearest,
ties-to-even; reject nonfinite results. Preserve finite amplitude without
normalizing it, including representable values outside [−1,1].

For integer bit depth B, use the symmetric scale `M = 2^(B-1)-1`. Validate the
undithered sample x is finite and in [−1,1]. Let `v = x*M` in binary64 and add
the dither count below, or zero for `none`. Reject if the resulting v is
nonfinite or outside [−M,M], including overload caused by dither. Quantize by
rounding v to the nearest integer with exact halfway cases away from zero;
never clamp. Store the signed result as two's-complement little-endian bytes
(three bytes for PCM24). Without dither, −1 and +1 become −32767/+32767 or
−8388607/+8388607. This preserves existing PCM16 quantization semantics.

The TPDF identity is `maac.dither.sha256-tpdf/1`. For each target, frame n,
channel c, and draw j (0 then 1), hash the following exact UTF-8 byte string:

```
maac.dither.sha256-tpdf/1\n<seed>\n<delivery_id>\n<target_id>\n<n>\n<c>\n<j>\n
```

Here `\n` denotes one LF byte; angle-bracketed fields are replaced without the
brackets. Integers are unsigned canonical decimal with no leading zeros except
zero itself; frame and channel are zero-based. Delivery/target IDs have the
schema's ASCII identifier form, so they cannot contain delimiters. Take the
first eight SHA-256 digest bytes as a big-endian unsigned integer, shift right
11, and divide by `2^53` to obtain the exactly representable uniform value
`u_j` in [0,1). Dither in integer-count units is `u_0-u_1`, added once to v.
It has triangular support (−1,1) LSB, zero theoretical mean, and no noise
shaping. Seed, IDs, frame, and channel fully determine it; processing block
size, selection order, other targets, and retry history do not advance a shared
random stream. Both subtractions/additions use binary64 nearest ties-to-even.

The artifact identity records encoding, channel order, converter identity,
rate, arithmetic mode, dither identity (`none` or the above exact identity),
and seed (null for none), as well as PCM and file hashes. Integer reconstruction
for analysis follows the WAV convention `signed_integer / 2^(B-1)`, **not** the
encoder's symmetric M. Thus encoded PCM16 +32767 meters as 32767/32768, and
encoded PCM24 +8388607 as 8388607/8388608. Float32 analysis reads the final
stored Float32 values exactly into binary64.

## 9. Authoritative final-artifact analysis

The analyzer identity is `maac.analysis.bs1770-5/2`, with the K-weighting,
gating, and deterministic true-peak profile below. Its normative external
reference is [ITU-R BS.1770-5, November 2023](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en).
It reports integrated LUFS, maximum sample peak, and maximum true peak. Loudness
range and a full EBU Mode claim are deferred. This profile fixes choices beyond
the recommendation so multiple implementations can be compared; it is not a
claim that the repository already passes official meter acceptance tests.

Analyzer revision 2 replaces revision 1's two-stage, sixteen-times true-peak
profile with the single-stage profile below. K-weighting and loudness gating
are unchanged. This revision changes analysis, dependent check results, and
analysis/render identities; it does not change rendering, encoding, or the
encoded audio bytes. Historical manifests retain their original analyzer and
true-peak identities and must not be relabeled as revision 2.

Analyze samples reconstructed from each successfully encoded final artifact,
after resampling and dither, using the encoding reconstruction in section 8.
Analyzing an earlier floating-point buffer is not authoritative. Reset analyzer
state independently for each complete artifact. Use binary64 nearest ties-to-
even, ordered multiply then add without FMA/reassociation or denormal flushing;
record implementation/build, coefficient identity, platform, math-library,
rounding mode, and relevant numerical environment. Nonfinite input or internal
results fail analysis; logarithm of zero is handled by status, never serialized
as NaN or infinity.

### 9.1 Rate-correct K-weighting

For each channel, cascade the high-shelf stage then the high-pass stage using
direct form I, normalized a0, and zero histories. These are the published
48000 Hz decimal coefficients; interpret the decimal constants exactly before
binary64 conversion:

| Stage | b0 | b1 | b2 | a0 | a1 | a2 |
| --- | --- | --- | --- | --- | --- | --- |
| Shelf | 1.53512485958697 | -2.69169618940638 | 1.19839281085285 | 1 | -1.69065929318241 | 0.73248077421585 |
| High-pass | 1 | -2 | 1 | 1 | -1.99004745483398 | 0.99007225036621 |

At 48000 Hz use these literals directly. For 44100 or 96000 Hz, this MaaC
profile fixes the following bilinear reconstruction from the published 48 kHz
filter; the ITU recommendation does not prescribe these particular coefficient-
generation steps. With `F0 = 48000`, apply to each numerator and denominator
triple `(c0,c1,c2)` separately:

```
A0 = c0+c1+c2
A1 = (c0-c2)/F0
A2 = (c0-c1+c2)/(4*F0^2)
C0 = A0 + 2*F*A1 + 4*F^2*A2
C1 = 2*A0 - 8*F^2*A2
C2 = A0 - 2*F*A1 + 4*F^2*A2
```

Here F is the artifact rate in Hz. Divide both resulting triples by the
resulting denominator C0. Evaluate these transformations as exact rational
operations and round each final normalized coefficient once to binary64,
nearest ties-to-even. Do not introduce a second prewarp or substitute a
library's approximate filter design. Use the same DFI equation and ascending
term order as EQ. Mono has one channel weight of 1; stereo has two weights of
1. Do not duplicate a mono channel or add surround/LFE weights.

### 9.2 Integrated loudness

Let F be the artifact rate. Use exactly `L = 2*F/5` frames per 400 ms block and
`H = F/10` frames per 100 ms hop (all supported rates make these integers).
Blocks begin at `0,H,2H,...` and are included only when the entire block lies
inside the artifact. Do not pad incomplete final loudness blocks. K-weighting
runs continuously from artifact frame zero; do not reset at block boundaries
or extend loudness analysis with filter flushing.

For each complete block j, compute each channel's mean square of K-weighted
samples over L frames, in ascending frame order, and sum channel energies in
channel order to obtain E_j. For E_j positive its block loudness is
`l_j = -0.691 + 10*log10(E_j)` LUFS. E_j zero is below every finite gate and
requires no evaluation of log zero.

The absolute gate retains blocks with `l_j > -70` (strict comparison). If this
set A is empty, integrated loudness is unmeasurable. Otherwise calculate
`l_A = -0.691 + 10*log10(sum(j in A,E_j)/count(A))` and relative threshold
`l_A-10`. The final set retains the blocks in A also satisfying
`l_j > l_A-10`, again strict. Integrated loudness is
`-0.691 + 10*log10(sum(E_j in final set)/count(final set))`. Sum retained
blocks in increasing start-frame order. Gates compare the computed unrounded
binary64 values, never report/display-rounded decimals.

### 9.3 Sample and true peaks

Sample peak amplitude is the maximum absolute reconstructed sample over every
frame and channel, before K-weighting. For a positive maximum P report
`20*log10(P)` dBFS. Silence has amplitude zero and a null logarithmic value.

The true-peak subprofile is `maac.truepeak.bs1770-5.annex2-4x/1`. It uses one
Annex 2 four-times interpolator with twelve taps per phase. For input x with N
frames, it produces `v[4*n+p] = sum(k=0..11,h[k,p]*x[n-k])`, with phase p
from 0 through 3 and n from 0 through N+10 inclusive. Samples outside the
input interval are zero; the stage therefore produces exactly `4*(N+11)`
frames and includes its complete FIR tail. Reset the stage to zero for each
artifact. Sum k in ascending order, independently for each channel. There is
no second interpolation stage.

The following exact binary fractions, written as decimals, are the Annex 2
coefficients. Each row is k; columns are phases 0, 1, 2, 3:

```
 0.001708984375  -0.0291748046875 -0.0189208984375 -0.00830078125
 0.010986328125   0.029296875      0.0330810546875  0.014892578125
-0.0196533203125 -0.0517578125    -0.0582275390625 -0.026611328125
 0.033203125     0.089111328125   0.1015625        0.047607421875
-0.0594482421875 -0.16650390625   -0.2003173828125 -0.102294921875
 0.1373291015625  0.465087890625   0.77978515625    0.97216796875
 0.97216796875    0.77978515625    0.465087890625   0.1373291015625
-0.102294921875  -0.2003173828125 -0.16650390625   -0.0594482421875
 0.047607421875   0.1015625        0.089111328125   0.033203125
-0.026611328125  -0.0582275390625 -0.0517578125    -0.0196533203125
 0.014892578125   0.0330810546875  0.029296875      0.010986328125
-0.00830078125   -0.0189208984375 -0.0291748046875  0.001708984375
```

For floating-point processing, omit the Annex 2 fixed-point headroom
attenuation/restoration pair; do not multiply phase results by four. Do not
renormalize these coefficients. Let T be the larger of the original sample
peak and the maximum absolute sample across the complete interpolator output,
including its flushed tail. Report `20*log10(T)` dBTP when T is
positive. The original sample peak is a mandatory lower bound. No K-weighting,
gating, per-block maximum reset, delay cropping, or extra gain applies to true
peak. This finite interpolator is a specified estimate, not an exact
continuous-time reconstruction guarantee.

For a unit impulse, the largest interpolated magnitude is exactly
`1991/2048 = 0.97216796875`; the original-sample lower bound makes the reported
true-peak amplitude exactly 1 (0 dBTP). A one-frame artifact produces 48
interpolated frames, including the complete tail.

The Rust delivery implementation conservatively charges analyzer work as
`channels * (64*N + 24*4*(N+11))`: 64 units per original frame for K-weighting,
loudness, and sample-peak overhead, plus twelve multiplies and twelve additions
per interpolated output. The charge includes all 44 flushed outputs even for
an empty artifact. Checked preflight accounting adds this work to SRC and
spool work under the caller's delivery budget; this analyzer revision does not
change those other charges or resource caps.

### 9.4 Statuses and limits

Status names below are JSON strings in machine-readable reports; `null` is a
JSON null, not a new MaaC source literal. Each peak report contains
`status = measured` with finite positive
`amplitude` and a finite logarithmic `value`, or `status = digital_silence` with
amplitude zero and `value = null`. Each loudness report contains
`status = measured` with finite LUFS `value`, or `status = unmeasurable` with
`value = null` and one reason: `digital_silence`, `insufficient_duration` (no
complete 400 ms block), or `below_absolute_gate`. Check silence first, then
duration, then gating. A defensive empty final gate is also unmeasurable with
reason `below_relative_gate`. Reports include an explicit metric unit; no
numeric infinity or NaN is permitted anywhere in the manifest.

Optional limits use existing record syntax with explicit unit symbols and
finite dimensionless exact numeric bounds, not new language suffixes:

```maac
limits = {
  integrated_loudness = { unit = LUFS; min = -20; max = -14; };
  sample_peak = { unit = dBFS; max = -1; };
  true_peak = { unit = dBTP; max = -1; };
};
```

These values illustrate syntax, not recommended or universal mastering targets.
`integrated_loudness` requires at least one of `min` and `max`, and when both
exist min MUST be less than or equal to max. Peak limits require only `max`;
`min` and unknown fields are forbidden. Wrong units, nonnumeric bounds,
nonfinite binary64 conversions, and malformed limits are errors. Omitted or
empty `limits` imposes no checks. There is no prescribed fixed range for a
valid finite limit value.

Limits compare unrounded final-artifact measurements inclusively: measured
value >= min and <= max. Unmeasurable requested loudness fails that check.
Digital silence passes every finite peak upper bound by its amplitude-zero
status; do not turn null into zero dB. Limits never invoke automatic gain
correction, normalization, limiting, or a second export.

## 10. Results and publication

Artifact completion and check results are separate. Each target result records
its IDs, selected port, role, encoding, rate, channels/order, frame count,
exact interval, all required converter/dither/analyzer identities, file and PCM
hashes, final-artifact measurements, and limit results. A completed artifact has
`artifact_status = complete`. Check status is `pass`, `fail`, or `not_requested`;
record each requested comparison's outcome and reason separately. The overall
check fails if any selected target check fails. A successful render with an
unmeasurable requested LUFS limit therefore returns failed check status.

Publish successfully rendered audio even when requested limits fail. Such
failure MUST NOT delete the artifact or leave it only in temporary storage.
Rendering/encoding errors are distinct from completed audio with failed limits;
analysis failure is likewise distinct and records its diagnostic without
invented measurements. Missing required measurements fail requested checks;
rendering, encoding, analysis, or publication errors also fail the job even
when no limits were requested. Preserve any successfully completed audio for
review.

Before publishing, obey the caller's overwrite policy and destination mapping.
Existing destinations MUST be protected unless overwrite was authorized by
that caller. Publish each completed artifact atomically, using a completed
file on the destination filesystem. Rendering and final-artifact analysis must
finish before a complete manifest is published atomically. A failure report
may describe a partial job but cannot claim that manifest or missing artifacts
are complete. There is no promised atomic transaction across all target files:
if publication fails after another target has been published, report the
actual per-target completion and retain those completed files. Never silently
replace an existing destination merely because a limit check has failed.

The CLI's caller-side mapping uses bounded, stable SHA-256 filename suffixes for
uppercase/mixed-case IDs and long components, independently of target selection.
The [CLI reference](reference.md) specifies the mapping and collision refusal;
full source IDs remain authoritative in delivery data and manifests.

## 11. Conformance and implementation gates

The [complete example](../examples/production.maac) uses the repository as its
package root; its descriptor asset path `production.schema.json` resolves from
that root. The schema is self-contained and its exact bytes must match the
example's asset hash. The example runs through the experimental implementation:

```sh
maac deliver examples/production.maac --project-root . --profile song --delivery release_cd --output-dir production-output
```

Its requested limits are illustrative, and completed audio is retained if they
fail. This runnable example does not qualify the current metering profile or
replace the acceptance gates below.

[`check_production.py`](../check_production.py) and
[`production-conformance.json`](../production-conformance.json) validate bounded
syntax/schema and selected semantics and arithmetic. Independent expected
results cover EQ impulses, compressor knees and time constants, the reverb's
first wet impulse, symmetric integer quantization and deterministic dither,
exact delivery frame counts, and the four-times true-peak impulse, adjacent
samples, silence, and complete tail counts. Invalid cases cover capabilities, units,
fields, references, sidechains, formats, rates, and limits. These checks do not
establish Rust renderer conformance or change performance-plan versions.
The Rust implementation has separate native, delivery, and resource tests;
external metering qualification is tracked in the evidence report.

Before advertising renderer support, implementations MUST additionally prove:

- Full graph context survives master/stem selection, including external
  sidechains, shared effects, automation, and nonlinear processing.
- Targets align at reset origin and have the directly calculated exact-duration
  frame counts; processing and state remain continuous through declared tails.
- Reset replay and automation retain the specified histories and deterministic
  target-addressed dither, independently of target-selection order.
- Analysis reads final encoded artifacts and meets applicable official ITU/EBU
  metering fixtures at supported rates, including gating, silence, short files,
  phase-sensitive true peaks, and complete filter tails.
- Independent SRC tests establish coefficient generation, alignment, response,
  boundary behavior, and alias rejection; exploratory sampled response alone
  cannot establish conformance.
- Requested check failures retain completed audio, fail the overall check,
  protect existing destinations, and truthfully report partial publication.

Passing arithmetic checks establishes only their stated numerical properties.
Acoustic realism, musical usefulness, and listening quality require separate
listening evaluation. No professional sound-quality claim follows solely from
this revision's specification or numerical fixtures.
