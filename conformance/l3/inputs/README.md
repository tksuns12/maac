# L3 core input policy corpus

This directory contains a fixed MaaC/1 matrix for core processor input cardinality. It covers exactly one audio input for `core.onepole/1`, `core.gain/1`, `core.fader/1`, `core.matrix/1`, `core.delay/1`, and `core.pan/1`; zero, one, and multiple inputs for `core.sum/1`; empty event inputs for `core.sine/1` and `core.kit/1`; and the absence of input ports on `core.constant/1`, `core.lfo/1`, and `core.noise/1`.

`expected.json` fixes the case inventory, diagnostic paths, retained incoming connection counts, empty event targets, and selected rendered samples. Successful cases compile through `SourceBundle` and `compile_bundle_artifact`, render through the public artifact renderer, and replay bit-for-bit after `PlanArtifact` JSON retention. The checked absolute `1e-12` sample bound applies only to the independently specified observations in this corpus.

The aggregate fixture uses the pinned one-frame mono PCM asset only to construct a valid empty-event `core.kit/1`; the asset is never triggered. The matrix checks cardinality and public integration behavior. Connection-ID summation order is covered by the separate A4 regression.
