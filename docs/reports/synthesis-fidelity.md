# Synthesis fidelity and FM spectral evidence

MaaC renders at 48 kHz. An oscillator emits the value at its current phase,
then advances phase by the current sample's evaluated frequency divided by
48,000 and wraps modulo one cycle. A zero frequency therefore holds phase, and
a negative frequency moves phase backward. Reset phase is captured at note-on
for voice oscillators; phase `1` is the same reset point as phase `0`.

Graph modulation is direct, sample-wise linear frequency modulation. At every
sample, the already rendered mono modulation source is multiplied by its signed
depth and added to the target frequency. The complete instantaneous frequency
must remain in the inclusive range -24,000 through 24,000 Hz. A value outside
that range is an error; the renderer does not clip or reduce modulation depth.

This behavior follows the direct linear FM, frequency/phase integration, and
through-zero discussion in Kasper Nielsen's
[DAFx-20 paper](https://karmafx.net/docs/karmafx_DAFx2020_paper_61.pdf),
especially Sections 2, 2.2, 3, and 5. The paper also explains that FM sidebands
can cross Nyquist and fold into the audible band. Its oversampling discussion
describes one possible mitigation; MaaC/1 does not implement or claim
oversampling.

## Table playback

Basic oscillators use 2,048-sample cyclic tables. Phase lookup linearly
interpolates the two adjacent samples, including the last-to-first boundary.
Their harmonic ceilings are powers of two; the richest legal ceiling satisfying
`ceiling * abs(frequency) <= 24,000` is selected from the current sample's
frequency. A zero frequency selects the richest table. A bank change is
immediate, with no interpolation or crossfade between harmonic banks.

Embedded wavetables use the same phase wrapping and cyclic linear interpolation.
Their normalized position linearly blends adjacent wavetable frames, while the
harmonic bank itself still changes immediately. Band-limiting the carrier table
does not remove sidebands created by later phase evolution, so strong FM may
still alias.

## Automated reference

The deterministic regression in `tests/fm_spectral.rs` measures one second of
output with a rectangular, coherent-bin DFT. Every test frequency is an integer
number of cycles in 48,000 samples, so no analysis window or peak interpolation
is needed. A reported amplitude is
`2 * hypot(real, imaginary) / 48,000` at the named positive-frequency bin.

The independent discrete reference uses

```text
m[n]       = sin(2 pi fm n / Fs)
x[n]       = sin(2 pi phase[n])
phase[n+1] = wrap(phase[n] + (fc + depth m[n]) / Fs)
```

It does not call MaaC oscillator or graph code. Summing those discrete frequency
increments gives the modulation index

```text
beta_d = pi * depth / (Fs * sin(pi * fm / Fs)).
```

For coherent sinusoidal FM, the predicted magnitude of sideband order `k` is
`abs(Jk(beta_d))`. The test evaluates `Jk` with an independent convergent power
series and checks that implementation against the known `J0(1)` and `J1(1)`
values before using it as an oracle.

The spectral tolerance is `5e-6` in linear amplitude. A unit sine lookup's
worst-case local linear-interpolation error is approximately
`(2 pi / 2048)^2 / 8 = 1.177e-6`. The allowance covers both modulator and carrier
lookups plus their phase accumulation. The observed full-stream errors below
remain under `1.9e-6`.

### Low-frequency fixture

The low-frequency case uses `fc = 4,000 Hz`, `fm = 400 Hz`, and
`depth = 400 Hz`. Its instantaneous frequency stays between 3,600 and 4,400 Hz,
and `beta_d = 1.000114240667`.

| Component | Predicted magnitude | Primitive measured | Instrument graph measured |
| --- | ---: | ---: | ---: |
| Carrier, 4,000 Hz | 0.765147412764 | 0.765147155873 | 0.765147155873 |
| Lower order 1, 3,600 Hz | 0.440087728645 | 0.440087103838 | 0.440087103838 |
| Upper order 1, 4,400 Hz | 0.440087728645 | 0.440087174781 | 0.440087174781 |
| Lower order 2, 3,200 Hz | 0.114927504180 | 0.114927280207 | 0.114927280207 |
| Upper order 2, 4,800 Hz | 0.114927504180 | 0.114927246945 | 0.114927246945 |

The primitive's largest time-domain difference from the independently
integrated ideal-sine reference was `1.891e-6`. Matching nonzero upper and lower
sidebands in the `InstrumentRuntime` result demonstrates that the compiled graph
applies modulation each sample. A separate graph fixture with a 23,000 Hz base
and 2,000 Hz depth fails with `E_NONFINITE` when the evaluated frequency exceeds
24,000 Hz, confirming error behavior rather than silent clamping.

### Through-zero fixture

An exact phase sequence begins at one-quarter cycle and applies frequencies
`0, -12,000, 0, +12,000, 0 Hz`. Its outputs are `1, 1, 0, 0, 1`: zero holds the
current phase, negative frequency reverses it, and positive frequency advances
it again. A second fixture uses `fc = 100 Hz`, `fm = 400 Hz`, and
`depth = 400 Hz`, so instantaneous frequency crosses zero and spans -300 through
500 Hz. Its maximum difference from the independently integrated reference was
`1.760e-6`.

### Residual-alias fixture

The strong case uses `fc = 10,000 Hz`, `fm = 9,000 Hz`, and
`depth = 6,000 Hz`. Instantaneous frequency remains valid at 4,000 through
16,000 Hz, but its upper second-order sideband is at 28,000 Hz. Sampling folds
that component to 20,000 Hz. With `beta_d = 0.706839672753`, the independently
predicted `abs(J2(beta_d))` magnitude is `0.059892818015`; the measured 20 kHz
peak is `0.059892865450` (`-24.4525 dB` relative to unit amplitude), a difference
of about `4.74e-8`. Repeated renders are bit-identical in the test.

These are formula checks and automated numerical measurements. They characterize
the implemented renderer and its residual aliasing; they are not perceptual
quality judgments, and no human listening test was performed.
