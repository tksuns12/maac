# L2 timing corpus

This directory contains 16 fixed MaaC/1 source cases and one two-frame PCM asset. `expected.json` records literal timing outcomes derived from the specification's exact arithmetic. The Rust integration test compiles each source through `SourceBundle` and `compile_bundle_artifact`, renders every successful artifact, and repeats the render after `PlanArtifact` JSON retention.

Automation fixtures route one shared sine signal to two output channels. `ratio` means channel 0 divided by channel 1 at the named output-local frame, so the shared oscillator and envelope cancel and expose the automated gain. Rational strings are exact expected values; the test uses an absolute tolerance of `1e-12` only for these rendered ratio and PCM observations. It is not a language-wide numerical tolerance.

The corpus is a bounded check of score/seconds automation origins and score-end clamping, audio reset-relative phase and admission, and note gate timing. It is not a universal numerical corpus or a claim that every processor, profile, or timing combination is covered.
