# MaaC documentation

This folder documents how to use MaaC. [`design/`](design/) records why it
works the way it does, and [`reports/`](reports/) records how each feature
was checked. The [project README](../README.md) has the install steps and a
first example.

## Getting started

- [Quick start](quickstart.md): install, check, compile and render.
- [Authoring a first score](tutorial.md): the language, step by step.
- [Feature tour](features.md): what MaaC can do, with a runnable example for
  each feature.
- [AI producer guide](ai-producer-guide.md): writing, measuring and
  delivering a whole track, as an AI model or anyone working without ears on
  the render.

## Writing music

- [Pattern audio](pattern-audio.md): audio leaves inside patterns.
- [Tempo ramps](tempo-ramps.md): accelerating and slowing down.
- Per-note expression on overlapping notes:
  [pitch](pitch-expression.md), [gain](gain-expression.md),
  [timbre](timbre-expression.md) and [pressure](pressure-expression.md).
- [Materializing a placement](materialize-instance.md): turning a placed
  pattern into editable notes.
- [Project entrypoint](project-entrypoint.md): `main.maac` projects and the
  song resource profile.

## Instruments and sound

- [Studio instruments](studio-instruments.md): electric and acoustic piano,
  electric guitar, bass, pad and drum kit.
- [Acoustic guitars](acoustic-guitars.md): steel, nylon and muted guitars.
- [Basic instruments](basic-instruments.md): 24 general-purpose sounds.
- [Reusable instruments and sound libraries](instruments.md): writing your
  own instruments, and the reference for every graph processor.
- [Per-note pitch](instrument-pitch.md) and [gain](instrument-gain.md) on
  reusable instruments.
- [Instrument-internal event modulation](internal-event-modulation.md) and
  [core modulation](core-modulation.md).
- [Plucked string](plucked-string.md): the `synth.pluck/1` contract.

## Recorded audio

- [Sample kits](core-kit.md), [audio clips](audio-clips.md) and
  [warp-rate clips](warp-rate.md).
- [WAV import](media-import.md) and [disk media](disk-media.md).
- [Take groups and comping](takes-and-comping.md), and
  [synchronized microphone takes](grouped-takes.md).

## Mixing, delivery and analysis

- [Production](production.md): EQ, compression, reverb, the limiter and
  `maac deliver`.
- Core processors: [fader](core-fader.md),
  [matrix](core-matrix.md), [delay](core-delay.md) and
  [noise](core-noise.md).
- [`maac analyze`](analyze.md): loudness, spectra and findings, with images.
- [Range export](render-range.md): rendering an excerpt.

## Reference

- [CLI and library reference](reference.md)
- [Diagnostics](diagnostics.md)
- [Capability matrix](capabilities.md): every supported feature and its
  limits.
- [Performance-plan format](performance-plan.md)
- The [MaaC/1 specification](../MaaC-1-Specification.md)

## Editing, archives and interchange

- [Editing kernel](editing-kernel.md) and
  [editing diagnostics](editing-diagnostics.md)
- [Composition archives](editable-archive.md)
- [Musical module artifacts](musical-module-artifact.md)
- [Generic interchange](generic-interchange.md) and
  [loss reports](interchange-loss-report.md)
- [Execution identity for events](execution-event-normalization.md) and
  [windowed event queries](windowed-event-query.md)
- [Quantitative conformance](quantitative-conformance.md)

## Design and evidence

- [`design/`](design/): the [design principles](design/design-principles.md),
  the [roadmap](design/language-specification-plan.md), and the proposals
  and decisions behind each feature.
- [`reports/`](reports/): delivery and validation reports, the
  [verification report](reports/verification.md), and the AI production
  trials ([T2](reports/ai-production-trial.md),
  [T3](reports/ai-production-trial-t3.md)).
