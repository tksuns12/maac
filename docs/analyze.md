# `maac analyze`: listening tools for AI producers

**Status:** accepted and implemented 2026-10-03 as tooling slice T1. The
owner accepted every recommendation in [Decisions](#decisions); the
[implementation notes](#implementation-notes) record where v1 differs from
the proposal text, which is kept below as written.

```sh
maac analyze examples/showcase/showcase.maac --project-root . --disk-media --profile song --images analysis/
maac analyze main.maac --json --section bars:8 --source pad:out
maac analyze main.maac --window region:b --json            # one section, exact
maac analyze main.maac --window bars:13-16 --preroll 4     # quick preview
maac analyze main.maac --json --notes                      # every note's checks
maac analyze main.maac --section regions --lead lead:out   # is the melody on top?
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

## Profile `maac.analyze.default/2` (2026-10-04)

The [T2 production trial](reports/ai-production-trial.md) found five problems in v1.
The profile identifier changed with them; the report schema did not.

- **Source roles:** a generator before its gain stage was reported as a mix
  source, and its full-scale noise "dominated" every band. Roles now follow
  the graph:
  - `output`;
  - `bus`: a mix point with two or more inputs, or any signal fed from one;
  - `stem`: an independent source feeding a mix point or the output;
  - `chain`: an earlier stage of a source.

  `band_dominance` compares stems, and `silent_source` reports stems.
- **Panning:** `stereo_imbalance` fired for single instruments that were
  panned on purpose. It now applies to the output and buses only.
  `mono_cancellation` still applies to every stereo source.
- **Loudness floor:** decaying tails produced short-term values such as -1751
  LUFS. As with the BS.1770 absolute gate, momentary and short-term maxima
  below -70 LUFS are now unmeasured.
- **Groove grids:** the trial's worst problem was found by the owner's ears,
  not by the tools. The hats swung their off-beat eighths (x + 2/3q) while
  keys, bass, kick and melody played them straight (x + 1/2q), 111 ms apart at
  90 bpm. The new `groove_mismatch` warning reports a section where one part
  plays at least four straight off-beat eighths and another at least four
  swung ones. It names the part on the grid with fewer onsets.
- **Drums in the piano roll:** synthesized drums are all struck on C2, so
  kick, snare and hats shared one row. A part that strikes a single pitch at
  least four times now gets its own lane.

## Windows (2026-10-07)

`--window` measures one part of the piece instead of all of it:

- `region:ID` is a declared region;
- `bars:FIRST-LAST` is a range of whole bars from the meter, inclusive.

The window is cut to the score. The report gains a `window` object with its
score bounds and frames, `frames` and `seconds` count the window, every
measurement and finding covers the window only, and sections are the ones
that overlap it, cut at its edges. Positions keep their place in the piece:
a peak in bar 14 is still reported in bar 14. Images show the window.

By default a window is **exact**. Rendering still starts at the beginning,
because reverbs, delays, compressors and sounding notes carry state into the
window, but it stops at the window's end instead of the piece's. The frames
measured are the same frames a full render produces, so the numbers match
what a whole-piece analysis would find for that stretch, apart from the
100 ms measurement blocks now starting at the window's first frame. An early
window is cheap; a late one costs nearly a full render.

`--preroll SECONDS` trades exactness for speed. Rendering starts that many
seconds before the window, from reset state:

- every note that started earlier and is still sounding there starts again
  at that frame, at its velocity but with a fresh envelope;
- earlier hits and notes that have ended are dropped;
- reverb, delay and dynamics start empty.

A preroll at least as long as the longest reverb tail and release makes the
window close to a full render's, but not equal. The report marks it:
`window.approximate` is `true` and `window.render_start_frame` says where
rendering began. Use it to iterate on one section; confirm the final mix
without it.

## Profile `maac.analyze.default/3` (2026-10-11)

The [perception gaps proposal](design/perception-gaps-proposal.md), part A, adds a
noise floor for F13 of the [T2 trial](reports/ai-production-trial.md): a hiss 27 dB
under the music read as a constant "sssss", and no finding fired. The report
schema is unchanged; every source and section gains three measurements:

- `floor_lufs`: the loudness of the quietest tenth of the 100 ms blocks above
  −70 LUFS. It needs at least one second of such blocks.
- `block_range_lu`: the 95th percentile of block loudness minus the floor
  percentile. A steady source has a small range.
- `floor_bands_db`: the octave spectrum of the floor blocks.

The new `exposed_noise` warning fires for a stem in a section when:

1. no note drives it: no node that receives score events lies at or upstream
   of it;
2. it is steady: `block_range_lu` is at most 6 LU, and it is above −70 LUFS
   in at least 90% of the blocks;
3. in the output's floor blocks, it supplies at least 10% of its mix point's
   energy in an octave band within 30 dB of that mix point's strongest band;
4. its estimated loudness at the output in those blocks is above −50 LUFS.
   The estimate is its K-weighted share of the mix point times the output's
   energy, so compression after the mix point still counts.

The message names the stem, the band, the mix point and both levels. A
deliberate steady texture, such as a drone that no note plays, also fires;
it is a warning, so keep the texture if you meant it.

## Profile `maac.analyze.default/4` (2026-10-11)

Part B of the [perception gaps proposal](design/perception-gaps-proposal.md) compares
the score with the audio. Each note or hit whose target node is an analysed
source is measured over a window: a note from its onset to its release, at
most 2 s; a hit for 250 ms. Three warnings or notes can follow, each grouped
by part and section and naming the worst note's source address in `event`:

- `silent_note` (warning): the target's K-weighted loudness over the note is
  at most −70 LUFS.
- `buried_note` (info): in the octave band where the target is strongest over
  the note, it holds −20 dB or less of its mix point. A node playing a chord
  is measured as a whole, so a quiet note inside a loud chord on the same node
  is not reported.
- `pitch_mismatch` (warning): a note that no other note on its node overlaps,
  with no pitch expression, sounds more than 50 cents from its written pitch.
  Pitch comes from YIN over four periods after a 30 ms attack, searching an
  octave either side. An octave error reads ±1200. A sub-octave layer reads at
  the written pitch. A note whose best dip is above 0.03, which includes bells,
  toms, tines and other inharmonic sounds, is `unpitched` and not checked.

Every source that receives events gets a `notes` summary for the whole piece
and each section: measured, silent, buried, pitched, unpitched and
mismatched counts, and the median and largest pitch deviation. `--notes` adds
a `notes` list with each note's level, band share, cents, aperiodicity and
status.

## Profile `maac.analyze.default/5` (2026-10-12)

The [lead part proposal](design/lead-part-proposal.md) adds a lead role, for
F21 of the [T3 trial](reports/ai-production-trial-t3.md): a melody 11 dB
under its accompaniment raised no finding. `buried_note` holds every part to
the same floor, and nothing said which part was meant to lead.

```sh
maac analyze main.maac --section regions --lead lead:out
```

`--lead NODE:PORT` names a part meant to lead; repeat it for more than one.
A lead is a mono or stereo port that reaches the project output and is not
the output itself, and notes reach it from a node at or upstream of it. No
two leads may play the same notes. Otherwise `--lead` is `E_USAGE`, or
`E_REFERENCE` for an unknown port. `--lead` adds the lead and its mix point
to the analysed sources, so it also works with `--source`.

A lead is read at its **mix point**: the nearest one downstream and, of
several at the same distance, the one nearest the output. A lead that feeds
a bus and a reverb send is read at the bus.

Each lead note is measured from its onset to its release, at most 2 s. Its
**margin** is the lead's energy over the rest of the mix point, in the
octave band where the lead is strongest over the note, in dB:

- **The rest** is the mix point less every lead that feeds it, so two leads
  playing together are each compared with the accompaniment. The subtraction
  treats the parts as uncorrelated, and a lead's own reverb return counts as
  accompaniment.
- **A clear note:** when the rest holds at most 1% of the mix point's energy
  in that band, the note is `clear` and has no margin.
- **A silent note** has no margin; `silent_note` reports it.

The new `lead_under` warning fires for a lead in a section that holds at
least 4 of its notes, when their median margin is −8.5 dB or less. The owner
set the limit in a listening round: "Morning Ferry"'s lead still led at a
median margin of −7.2 dB and was buried at −10.1 dB. The finding names the
quietest note in `event`.

The report gains:

- `leads`: the named ports;
- in a lead source's `whole` and in each section, a `lead` object:
  `mix_point`, `notes`, `clear`, `median_db`, `min_db`, and `under`, the notes
  at or below the limit. `median_db` is the lower middle margin for an even
  count, with clear notes above every margin, and is absent when that note is
  clear;
- with `--notes`, a `lead_notes` list with each lead note's `event`, `lead`,
  `section`, `margin_db` and `status`: `ok`, `under`, `clear` or `silent`.

Without `--lead`, a report is the same as under `default/4` apart from the
profile identifier.

**Limits:**

- A lead needs its own node. A piano playing melody and chords on one node is
  measured as a whole.
- A lead routed with its harmonies into a sub-bus is compared with the
  harmonies only.
- A lead without notes, such as an audio clip, cannot be named.

## Profile `maac.analyze.default/6` (2026-10-13)

The [gain reduction proposal](design/gain-reduction-proposal.md) reports what
each compressor and limiter takes, for F20 of the
[T3 trial](reports/ai-production-trial-t3.md). In that trial the AI found that
a glue compressor and a limiter flattened a chorus only by comparing region
loudness by hand.

Each analysed source that is the output of an `fx.compressor/1` or an
`fx.limiter/1` gets a `dynamics` object, in `whole` and in each section:

- `reduction_db`: the loudness the processor takes, in dB. At each frame the
  processor's K-weighted output power is scaled back up by its gain
  reduction; the sum over the span is divided by the output power. It is
  absent when the output is silent.
- `max_reduction_db`: the largest gain reduction at any frame.

The renderer reads the gain reduction from the processor, without changing
the audio:

- for the compressor, the smoothed reduction it subtracts before its makeup;
- for the limiter, −20·log10 of its gain, leaving out its `gain` drive.

An automated makeup or limiter `gain` is therefore the author's level change,
not a reduction. A sidechain compressor shows how far it ducks. The other
measures add up 100 ms blocks, but `dynamics` sums each section's exact
frames, so a section's maximum does not take in the start of the next.

The **master chain** is the compressors and limiters between the last mix
point and the project output. The analyzer walks upstream from the output
through nodes with one main audio input, and stops at the first node with
several inputs or none. A compressor's sidechain does not count as an input.
The report lists the chain as `master_chain`, node IDs in signal order,
omitted when empty. With `--source`, the chain's processors are analysed too.
The human output adds a `reduction` line per processor, master chain first,
with its reduction per section.

A processor that takes more from the louder of two sections takes the
difference out of the lift between them. On the proposal's six songs,
`reduction_db` agrees with each processor's input and output loudness within
0.15 dB.

There is no finding. In the owner's
[listening round](design/gain-reduction-proposal.md#results-listening-round),
a chorus that kept none of its lift at the output still sounded fine, because
the arrangement carried it.

Without a compressor or limiter, a report is the same as under `default/5`
apart from the profile identifier.

## Profile `maac.analyze.default/7` (2026-10-13)

Two fixes from the [T3 trial](reports/ai-production-trial-t3.md). The report
schema is unchanged.

**Section edges (F24).** A section took the 100 ms blocks whose first frame
lay inside it, so its last block reached up to 100 ms into the next section.
A part that rests until the verse read as sounding in the intro, and
`band_dominance` reported it there. Now neither kind of measure takes in
another section's audio:

- Values that add up frames use the section's exact frames: RMS, crest,
  sample and true peak, clipped samples, the octave spectrum, the shares of
  the output and the stereo measures.
- Values that need whole blocks use the blocks that lie wholly inside the
  section: integrated, momentary and short-term loudness, the floor, the
  block range and activity. Up to 100 ms at each edge is left out of them.

Whole-piece values are unchanged. On the six songs of the
[gain reduction proposal](design/gain-reduction-proposal.md), integrated
loudness per section at the output moves by at most 0.04 LU. Maximum
short-term loudness moves by up to 0.3 LU, where a 3 s window used to reach
into the next section.

**Pitch over a ringing release (F25).** A note counts as lone when no other
note on its node overlaps it in the score, but the last note's release can
still ring under it. In T3's nylon melody the window then held two pitches,
and 64 of 132 notes read `unpitched`. The pitch estimate now first cancels
the pitches of earlier notes on the node released up to 1 s before the
estimate ends, at most the two most recent. Each is cancelled by a comb at
its period, which removes its harmonics and leaves the note's own period
intact. A ringing pitch the note equals, or one whose harmonic the note is,
cannot be cancelled without cancelling the note, so it is left in. On the
nylon melody all 132 notes are now pitched, within 0.5 cents of their
written pitch. On the same six songs, the count of unpitched notes fell or
stayed the same in every part, and no `pitch_mismatch` appeared.

## Profile `maac.analyze.default/8` (2026-10-14)

Part B of the [vibrato proposal](design/vibrato-proposal.md), for F29 of the
[T4 trial](reports/ai-production-trial-t4.md). The report schema is
unchanged.

**Notes with a pitch expression.** `pitch_mismatch` skipped every note with
a pitch expression, which in T4 left 41 of the melody's 178 notes, the bent
and held ones, unchecked. Such a note is now checked like any other, under
the same lone-note rule, against its expressed pitch: the written pitch
moved by the mean of the expression over the estimate window, from 30 ms
after the onset over four periods. Its `cents` is its distance from that
pitch, and the 50-cent limit is unchanged.

- **Too fast to check.** If the expression moves more than 50 cents within
  the window, as a fast slide does, the note is not checked.
- **What it confirms.** At 30 ms a bend usually still sits at its starting
  pitch, so the check confirms the note's pitch and the start of its curve,
  not the top of the bend. The curve is exact data the instrument follows,
  so a wrong root or a detuned oscillator shows at the start as anywhere
  else.
- **Ringing releases.** An earlier note's release is cancelled at the pitch
  it rings at: its written pitch moved by its expression's value at the end
  of its gate, which the release holds.

On "Lantern Street", all 178 melody notes now get a pitch estimate: 149 are
pitched, against 115, and 29 unpitched. The largest deviation is 1.0 cent,
and no `pitch_mismatch` appears. Notes without a pitch expression are
measured exactly as before.

## Deferred to later versions

- **Onset alignment:** whether written and sounding attacks line up.
  `groove_mismatch` covers the error the trial hit.
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
[design principles](design/design-principles.md#keep-general-computation-in-tools).
It adds a new **AI production tooling** track next to the language track,
which [the plan](design/language-specification-plan.md) should record once this is
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
  frame lies inside it, so its edges are exact to within one block. Since
  `default/7`, sections use their exact frames instead; see
  [that profile](#profile-maacanalyzedefault7-2026-10-13).
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
