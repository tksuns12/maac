# L3 contract corpora

The bounded L3 fixtures are split into three matrices:

- [`core.pan/1` raw and effective range](expected.json)
- [core processor input policy](inputs/README.md)
- [tuning reference indexes](tuning/expected.json)

## `core.pan/1` raw and effective range

The pan matrix contains eight fixed MaaC/1 sources. `expected.json` records independently specified retained-plan values, modulation order and amounts, diagnostics, and selected rendered stereo samples.

The successful fixtures feed a deterministic mono sine into `core.pan/1`. Authored base values and automation points remain raw exact rationals in the retained artifact. Rendering adds the base or current automation replacement to all modulation contributions, clamps the final effective value once to `[-1, 1]`, and then applies equal-power panning. The linear automation fixture observes the signal at exactly `3/4q`; clamping its raw endpoints before interpolation would produce a different stereo result.

The Rust integration test compiles through `SourceBundle` and `compile_bundle_artifact`, checks public artifact JSON, renders with the public artifact renderer, and repeats rendering after `PlanArtifact` JSON retention. Its absolute `1e-12` sample bound applies only to these fixed observations and does not define a general numerical tolerance or an L5 policy. This is a bounded `core.pan/1` corpus, not a complete processor or tuning conformance claim; `synth.pan/1` keeps its separate strict authored range.
