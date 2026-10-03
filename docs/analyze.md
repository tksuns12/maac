# `maac analyze`: listening tools for AI producers

**Status:** accepted and implemented 2026-10-03 as tooling slice T1. The
owner accepted every recommendation in [Decisions](#decisions); the
[implementation notes](#implementation-notes) record where v1 differs from
the proposal text, which is kept below as written.

```sh
maac analyze examples/showcase/showcase.maac --project-root . --disk-media --profile song --images analysis/
maac analyze main.maac --json --section bars:8 --source pad:out
```

## Goal

MaaC's central product goal is a framework in which an AI can produce a
finished track itself. An AI producer works in a loop: **write** source,
**render** it, **perceive** the result, **judge** it against a goal, and
**revise** the source. MaaC already supports writing, rendering and revising
well:

- exact, typed, deterministic text;
- reusable instruments and patterns;
- Protocol 2 transactions;
- event queries;
- §23 diagnostics that name the authored object and field.

Perception is the weakest step. The only built-in measurement is the
delivery analyzer, which gives one integrated loudness and two peaks for a
whole file. An AI has no ears, so for it measurement *is* listening.

The stereo showcase work on 2026-10-02 showed this gap directly. Two real
problems were found only with ad hoc numpy scripts written during the session,
not with `maac`:

- the finale clipped;
- a pad that shared a period with the keys sample summed coherently on the
  left channel only.

Those scripts measured per-section peak and RMS, left/right balance, stereo
correlation and fundamental frequency. That is the kind of report this
proposal makes a first-class tool.

## What MaaC can do that a generic analyzer cannot

A generic audio analyzer can say "too much low end at 1:12–1:25". The renderer
knows the score, the signal graph and every node's output, so `maac analyze`
can phrase a measurement in source terms:

> In region `chorus` (bars 33–48), node `pad` is 9 dB louder than node `lead`
> in the 200–400 Hz band.

An AI can act on that by editing `pad`. The parts this needs already exist:

- `render_ports_*_with_limits` renders several node output ports in one pass
  for every plan version.
- §19 `region` gives an exact, author-named answer to "which interval is the
  chorus".
- The tempo and meter maps convert bars and `q` positions to frames exactly.
- Score-window event queries return each event's source address and frames.
- The BS.1770-5 analyzer (`maac.analysis.bs1770-5/2`) supplies K-weighting,
  gating and true peak.

## Proposed v1

### Command

```sh
maac analyze [INPUT] [--section regions|bars:N|placements] [--source PORT]...
             [--images DIR] [--json] [--profile song] [--disk-media]
```

`INPUT` is a source file, project directory or retained plan, as for `build`.
The command renders the plan once, tapping the master output and every
analysed source port, and writes one report. It never changes the source.

### Sections

Every measurement is reported for the whole piece and for each section:

- `regions` (default when the source has any): one section per `region`,
  named by its ID and label.
- `bars:N`: consecutive blocks of N bars (default 4) from the meter map. This
  is the default when there are no regions.
- `placements`: one section per `place` span.

Section boundaries are exact frames, derived the same way rendering derives
them.

### Sources

By default the report analyses these sources:

- the project output (`master`);
- every node, audio clip or placement output port that reaches it through
  connections, named by its authored ID;
- each instrument node, which is also reported per track that targets it.

`--source` restricts or extends the set with explicit `&node:port`
references.

### Measurements

Each section × source gets the following measurements, all deterministic
binary64 on a single thread:

| Group | Measurements |
| --- | --- |
| Level | Integrated, maximum momentary (400 ms) and maximum short-term (3 s) loudness, all with BS.1770-5 K-weighting; RMS; crest factor |
| Peaks | Sample peak, true peak, count of samples at or above full scale, and the first clipped position as a bar, `q` and frame |
| Spectrum | Energy in ten fixed octave bands (31.5 Hz–16 kHz), from a deterministic biquad filter bank, as dB relative to the source's total |
| Stereo | Left/right RMS balance in dB, interchannel correlation, mid/side ratio, and mono-sum loss in dB |
| Activity | Silent and active fractions and leading or trailing silence; the threshold is fixed in the report |
| Share | Each source's share of the master's energy per band. This is an approximation, because processing after the tap (reverb, master gain) changes the sum |

### Findings

The report starts with a short `findings` list for an agent to read first. Each
finding gives:

- a kind, a severity and the measured values;
- a location: section, source ID, bar and `q`;
- the threshold that produced it.

The proposed v1 kinds are `clipping`, `true_peak_over`, `loudness_jump`
between adjacent sections, `stereo_imbalance`, `mono_cancellation`,
`band_dominance` (one source dominates a band in the mix), and `silent_source`
(a source that never sounds).

Thresholds come from a named, versioned profile, `maac.analyze.default/1`.
They are measurements against stated limits, not aesthetic judgments, which
keeps to the specification's rule that subjective instructions are not
executable.

### Report identity

The JSON schema is `maac.analysis-report/1`. The report carries:

- the plan's execution identity, so an agent knows which revision it heard;
- the analyzer and profile identifiers;
- the numerical environment.

The same plan and environment give byte-identical JSON. Cross-platform
identity is not claimed, matching the existing analyzer's limits.

### Images (optional in v1)

`--images DIR` writes PNG files that a multimodal model can look at:

- a log-frequency spectrogram per section for the master and each source;
- a piano roll of the resolved events, coloured by track and drawn on the
  same time axis.

Both images are drawn from data the renderer already has. The build is offline
with no image or FFT crate, so this needs a small radix-2 FFT and a minimal PNG
writer using stored deflate blocks, both implemented in the repository.

## Deferred to later versions

- **Score–audio agreement:** whether each note is audible in the mix, the
  pitch actually sounding against the written pitch for monophonic parts, and
  onset alignment.
- **Reference profiles:** comparing against a genre or reference track's
  loudness and spectral targets. A reference profile would be a pinned asset,
  not a built-in taste.
- **Masking between specific pairs** of sources beyond `band_dominance`.
- **An AI producer guide:** a workflow document covering brief, arrangement,
  sound, mix, master and delivery, with the commands and report fields to
  check at each step. This could start before `analyze` lands.

## What it does not change

The language does not change: no grammar, no source kinds, and no plan or
delivery formats. `analyze` is a tool in the sense of the
[design principles](design-principles.md#keep-general-computation-in-tools).
It adds a new **AI production tooling** track next to the language track,
which [the plan](language-specification-plan.md) should record once this is
accepted.

## Cost and risk

- **Cost:** medium. Most of the work is the measurement accumulators, section
  and source resolution, the findings profile, the FFT and PNG writer if images
  are in v1, and the tests.
- **Testing:** use synthesized fixtures with known answers, such as a sine at
  a known level, a hard-panned source, an inverted-phase pair and a clipped
  section, plus the stereo showcase as an end-to-end case.
- **Main risk:** findings thresholds that are too noisy or too quiet for
  agents. They are versioned so they can be tuned without changing the
  measurements.

## Decisions

The owner accepted every recommendation:

1. **v1 scope:** measurements, findings and images.
2. **Default sections:** regions when present, otherwise 4-bar blocks.
3. **Default sources:** every output port that reaches the project output.
4. **Findings:** included, from the `maac.analyze.default/1` profile.

## Implementation notes

[`src/analyze/`](../src/analyze/) implements v1, and
[`tests/analyze.rs`](../tests/analyze.rs) checks it on synthesized fixtures
with known answers: a sine's level, crest and octave; an equal-power pan's
balance; an inverted pair's mono cancellation; clipping at its bar; regions,
bar blocks and whole-piece mode; source discovery and a silent source;
byte-identical reports and images; and the CLI's JSON. v1 differs from the
proposal text as follows.

- **Measurement grid:** measurements aggregate 100 ms blocks on one grid that
  starts at the first rendered frame. A section takes the blocks whose first
  frame lies inside it, so its edges are exact to within one block.
- **Positions:** a position gives the frame, the seconds and the bar, with `q`
  at or before it on a quarter-note grid.
- **Source roles:** each source has a role, `output`, `bus` (it receives other
  sources) or `leaf`. `band_dominance` compares leaves only, because a bus
  always contains its inputs.
- **One image per source:** spectrograms cover the whole piece, one image per
  source, with section boundaries drawn and each section's pixel range listed
  in the report. This replaces one image per section and source, which gave
  an agent too many images to inspect.
- **Piano roll:** the piano roll shows notes by pitch and hits in lanes,
  coloured by target node, with a legend in the report. Audio clips appear only
  in spectrograms.
- **Placement sections:** the `placements` section mode is deferred, because
  placement spans need compiler data that plans do not keep.
- **Plan input:** for a retained plan there is no source meter, so bar blocks
  become 16q blocks and positions carry no bar.

Run on the stereo showcase (62.6 s, 18 sources, 19 images), v1 took 14.5 s in
a release build. It reported a master true peak of -0.86 dBTP, over the -1
dBTP limit, and the glass recording's intentional right-heavy balance. The
spectrogram and piano roll were readable by a multimodal model.
