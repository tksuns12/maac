# Showcase listening record

This page records informal user listening approvals. They are separate from
the automated test evidence and do not replace it.

## 2026-10-02: stereo showcase

| Field | Value |
|---|---|
| Recorded | 2026-10-02 |
| Listener | Project owner (tksuns12) |
| Source | [`examples/showcase/showcase.maac`](../examples/showcase/showcase.maac) at commit `aa15157` |
| Build | `maac build examples/showcase/showcase.maac --project-root . --disk-media --profile song -o showcase-stereo.wav --format pcm16` |
| Rendered file | 62.6 s, stereo, 48 kHz, 16-bit PCM WAV, `sha256:71ed6c1ff0670ea5c6ce5e8dbda0c9a9e5c52b66cc4bd01e5dcf773947675aaa` |
| Playback | Downloaded to an Android phone over the tailnet and played there; whether headphones were used was not stated |
| Verdict | Approved: "Great!" |

### What the render adds

- Stereo `synth.sample/1` playback: a stereo WAV pad with a sustain loop, its
  channels tuned to 259.5 and 262.3 Hz.
- One stereo PCM recording declared once as a composition `asset`. It is
  played as an audio clip at its own pitch, then as a pitched sampler through
  `sample glass_a4 { asset = &glass; root = A4; }`.
- The earlier sections are unchanged, copied to both channels through
  `core.matrix/1`. The reverb is stereo, and the master gain is 0.64 dB lower to
  keep the same headroom.

### Later revision (not listened to)

On 2026-10-04 `maac analyze` measured this render's master true peak at
-0.86 dBTP, over the -1 dBTP limit. The excess was only in the finale, where
the keys part was the loudest source. The finale's keys chords were softened
from velocity 7/10 to 1/2. The current render,
`sha256:e58ee5b38e561c3a7c8f895d38e332937b7b5c2807b473d3a3128a2b0932c3c3`,
peaks at -2.31 dBTP:

- bars 1–20 measure exactly as before;
- bar 24, where the finale begins, changes slightly;
- the finale is about 0.9 LU quieter.

The approval above covers the earlier render only; nobody has listened to
the revision yet.

### Scope

This is an overall, informal approval of the whole render. It is not:

- a judgment of the stereo image, which needs headphones;
- a level-matched or blind comparison with the mono render;
- producer acceptance;
- listening evidence for any other example or render.

The numerical checks made when the file was rendered are summarized in the
[examples README](../examples/README.md#feature-showcase).

## 2026-10-01: mono showcase

| Field | Value |
|---|---|
| Recorded | 2026-10-01 |
| Listener | Project owner (tksuns12) |
| Source | [`examples/showcase/showcase.maac`](../examples/showcase/showcase.maac) at commit `ce79fc8` |
| Build | `maac build examples/showcase/showcase.maac --project-root . --disk-media --profile song -o showcase.wav --format pcm16` |
| Rendered file | 53.0 s, mono, 48 kHz, 16-bit PCM WAV, `sha256:acbf7fc532115a02b65753706e533a016ab68c8a76ee284071ccee6bde86c4dd` |
| Playback | Downloaded to an Android phone over the tailnet and played there |
| Verdict | Approved: "I liked it" |

### What the render covers

- Pitched `synth.sample/1` playback over two octaves, with a sustain loop on a
  held chord.
- Velocity layers with an equal-power velocity crossfade.
- A key crossfade between two samples.
- Embedded WAV samples and asset-backed PCM samples.
- One recorded phrase played at its own speed, then with `warp_rate` and
  `warp_preserve` (`core.stretch.ola/1`), each at twice and half the speed.

### Scope

This was an overall, informal approval of the whole render on phone playback.
It is not:

- a level-matched or blind comparison;
- a judgment of individual sections, such as the quality of `warp_preserve`;
- producer acceptance;
- listening evidence for any other example or render.

The listener made no specific comments on the stretcher's artifacts.
