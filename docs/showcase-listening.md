# Showcase listening record

This page records one informal user listening approval. It is separate from
the automated test evidence and does not replace it.

| Field | Value |
|---|---|
| Recorded | 2026-10-01 |
| Listener | Project owner (tksuns12) |
| Source | [`examples/showcase/showcase.maac`](../examples/showcase/showcase.maac) at commit `ce79fc8` |
| Build | `maac build examples/showcase/showcase.maac --project-root . --disk-media --profile song -o showcase.wav --format pcm16` |
| Rendered file | 53.0 s, mono, 48 kHz, 16-bit PCM WAV, `sha256:acbf7fc532115a02b65753706e533a016ab68c8a76ee284071ccee6bde86c4dd` |
| Playback | Downloaded to an Android phone over the tailnet and played there |
| Verdict | Approved: "I liked it" |

## What the render covers

- Pitched `synth.sample/1` playback over two octaves, with a sustain loop on a
  held chord.
- Velocity layers with an equal-power velocity crossfade.
- A key crossfade between two samples.
- Embedded WAV samples and asset-backed PCM samples.
- One recorded phrase played at its own speed, then with `warp_rate` and
  `warp_preserve` (`core.stretch.ola/1`), each at twice and half the speed.

## Scope

This was an overall, informal approval of the whole render on phone playback.
It is not:

- a level-matched or blind comparison;
- a judgment of individual sections, such as the quality of `warp_preserve`;
- producer acceptance;
- listening evidence for any other example or render.

The listener made no specific comments on the stretcher's artifacts. The
numerical checks made when the file was rendered are summarized in the
[examples README](../examples/README.md#feature-showcase).
