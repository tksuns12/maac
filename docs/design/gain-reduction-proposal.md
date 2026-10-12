# Gain reduction per section: proposal

**Status:** accepted 2026-10-13; the owner accepted every recommendation in
[Decisions](#decisions). The listening round comes next. This is the second half of item 2 of
[what to build next](../reports/ai-production-trial-t3.md#what-to-build-next-in-order)
in the T3 trial: F20, gain reduction per section for the dynamics
processors. The first half, a lead role (F21), landed as the
[lead part proposal](lead-part-proposal.md).

## Why

In T3's first draft the chorus did not lift. The glue compressor and the
limiter on the master took most of the lift that the arrangement built.
[`maac analyze`](../analyze.md) reports loudness per source and section, but
nothing about what a compressor or limiter does, so the AI:

- compared the integrated loudness of `bus`, `glue` and `master` per region
  by hand;
- rendered the song twice more at different limiter gains to see how much of
  the lift the limiter took;
- accepted a version in which "only about 0.7 LU survives at the master" and
  moved on.

The measurements below show that this delivered draft still lost 71% of its
lift, and that the limiter took more of it than the glue did. No finding
said so (F20).

A mixing engineer reads this from a gain-reduction meter on each processor.
The analyzer can do better than a meter: it knows the sections, so it can say
how much each processor takes from each section and how much of a lift
between two sections it removes.

## Measurements behind this proposal

Six versions were measured. The three drafts are T3's folk-pop draft at the
end of iterations 2, 3 and 4, rebuilt from the trial's edits. Their
per-region loudness at `bus`, `glue` and `master` matches the trial's saved
reports to 0.01 LU.

| Version | What it is | `glue` (threshold, ratio, makeup) | `master` gain |
| --- | --- | --- | --- |
| draft 2 | Folk-pop draft, iteration 2 | −24 dB, 2, +2 dB | 3 dB |
| draft 3 | Iteration 3: strum level automated down in the verses | −18 dB, 2, 0 dB | 3 dB |
| draft 4 | Iteration 4, delivered for the owner's first verdict ("fine") | −18 dB, 1.5, 0 dB | 2 dB |
| t3 | The rock ballad as approved (commit `55c2ae8`): "Good. It's good." | −18 dB, 1.5, 0 dB | 2 dB |
| ferry | The released "Morning Ferry" | −18 dB, 1.5, 0 dB | 2 dB |
| lofi | "Late Window", approved in T2 | −26 dB, 3, +4 dB | 9.8 dB |

Every version runs one `fx.compressor/1`, `glue`, into one `fx.limiter/1`,
`master`, which is the project output. Each processor's input and output were
rendered and divided frame by frame. This recovers the gain each one applied
exactly, because both apply one gain to both channels.

### What each processor takes from each section

A processor's **reduction** over a section is the mean of its gain reduction
in dB, weighted by the loudness of its input, leaving out its makeup or
drive. This weighting makes the reduction equal to the loudness the processor
takes from the section. Weighted by the input's energy, it was checked in all
70 readings (35 sections, two processors) against the input's integrated
loudness plus makeup or drive, minus the output's. The two agree within
0.15 dB, with a median of 0.04 dB. A plain time average of the reduction in
dB is off by up to 0.88 dB.

Reduction per section, as mean / maximum in dB:

| Version | Processor | intro | verse1 | chorus1 | outro |
| --- | --- | --- | --- | --- | --- |
| draft 4 | `glue` | 0.32 / 0.79 | 1.60 / 2.87 | 2.09 / 3.23 | 0.62 / 1.61 |
| draft 4 | `master` | 0.00 / 0.00 | 0.24 / 2.03 | 0.93 / 4.23 | 0.00 / 0.02 |
| ferry | `glue` | 0.30 / 0.64 | 1.32 / 2.74 | 1.94 / 3.21 | 0.67 / 1.64 |
| ferry | `master` | 0.00 / 0.00 | 0.25 / 2.68 | 0.50 / 3.59 | 0.00 / 0.00 |

The second verse and chorus repeat the first: the means within 0.02 dB, the
maxima within 0.15 dB.

### How much of a lift the processors take

When the mix gets louder from one section to the next, a processor that
reduces more in the louder section takes the difference out of the lift. In
the table below:

- **Lift at the output** is the change in the output's integrated loudness.
- **Lift before the master chain** adds back what the processors took. On
  every rising pair of sections in these songs, it agrees with the lift
  measured at the glue's input within 0.2 LU.
- **Share** is how much of that lift the processors took.

| Version | Sections | Lift before the master chain | `glue` takes | `master` takes | Lift at the output | Share |
| --- | --- | --- | --- | --- | --- | --- |
| draft 2 | verse1 → chorus1 | 1.10 LU | 0.76 | 0.54 | −0.20 LU | 118% |
| draft 3 | verse1 → chorus1 | 1.65 LU | 0.74 | 0.67 | 0.24 LU | 85% |
| draft 4 | verse1 → chorus1 | 1.66 LU | 0.49 | 0.69 | 0.48 LU | 71% |
| t3 | verse1 → chorus1 | 2.47 LU | 0.60 | 0.07 | 1.80 LU | 27% |
| ferry | verse1 → chorus1 | 2.49 LU | 0.62 | 0.25 | 1.62 LU | 35% |
| lofi | intro → a | 2.89 LU | 1.35 | 0.15 | 1.39 LU | 52% |
| lofi | a → b | 1.38 LU | 0.31 | 0.33 | 0.74 LU | 46% |

The second verse-to-chorus change gives the same figures within 0.05 LU.

- **The drafts** keep under half a loudness unit of lift, or none. The trial's
  fixes did not change that. Draft 3 automated the strums, which raised the
  lift before the chain from 1.10 to 1.65 LU. Draft 4 then eased the glue and
  the limiter, and the output still kept only 0.48 LU.
- **The limiter is the larger cause in draft 4.** Its mean reduction is small
  (0.24 dB in the verse, 0.93 dB in the chorus), but almost all of it falls
  in the chorus, which is where the lift is. The glue reduces more overall and
  takes less of the lift. The trial found the limiter's part only by
  rendering the song at two more limiter gains.
- **The approved mixes** lose 27–52% and keep at least 1.39 LU, apart from
  "Late Window"'s `a` → `b`. That lift was small to begin with (1.38 LU), and
  the master chain took 46% of it, leaving 0.74 LU. No one has judged that
  change by ear.

## Proposed contract

This is tooling only. Nothing changes in the grammar, source kinds, plans,
delivery formats or rendered audio.

### Reduction per processor

Each analysed source that is the output of an `fx.compressor/1` or an
`fx.limiter/1` gets a `dynamics` object, in `whole` and in each section:

- `reduction_db`: the mean gain reduction, weighted by the input's loudness,
  as [measured above](#what-each-processor-takes-from-each-section);
- `max_reduction_db`: the largest gain reduction at any frame.

The reduction is the processor's own state, read during the analysis render
(see [where the reduction comes from](#where-the-reduction-comes-from)):

- **For the compressor**, it is the smoothed reduction *r* in dB that it
  subtracts before its makeup.
- **For the limiter**, it is −20·log10 *g*, with *g* its gain at the frame.

Neither counts the makeup or the limiter's `gain` (its drive). An automated
makeup is therefore the author's level change, not a reduction. A sidechain
compressor's reduction follows its detector, so a ducked pad shows how far it
ducks.

The weighting uses the K-weighted power of the processor's output, scaled
back up by its reduction at each frame. The input port does not need to be
analysed.

### The master chain

The **master chain** is the dynamics processors between the last mix point and
the project output. The analyzer walks upstream from the output through nodes
with one main audio input and stops at the first node with several inputs or
none. A compressor's sidechain input does not count as one. In all three songs
the chain is `glue`, then `master`. A compressor on a stem or a sub-bus is
reported, but it is not in the chain. The chain's processors are added to
the analysed sources, as `--lead` adds a lead's mix point.

### Finding `lift_lost`

This is a **warning** for two consecutive sections, A then B, in which the mix
gets louder. Let:

- *lift at the output* be B's integrated loudness at the output minus A's;
- *taken* be the sum, over the master chain, of each processor's
  `reduction_db` in B minus its `reduction_db` in A;
- *lift before the chain* be the lift at the output plus *taken*.

The finding fires when all three hold:

1. **The arrangement lifts:** the lift before the chain is at least 1 LU.
2. **The output does not:** the lift at the output is under 1 LU.
3. **The master chain is the main cause:** *taken* is at least half the lift
   before the chain.

> From `verse1` to `chorus1` the mix rises 1.66 LU before the master chain
> but 0.48 LU at the output (limit 1 LU): `master` takes 0.69 LU of the lift
> and `glue` 0.49 LU.

Its `value` is the lift at the output and `threshold` is 1. `section` and `at`
name B, and the message lists the processors, largest first.

- **1 LU, both ways:** a lift under 1 LU is hard to hear as one. A lift the
  arrangement did not build is not the dynamics' fault, so rule 1 uses the
  same limit. It is a starting value; see [decision 3](#decisions).
- **Half:** when the arrangement's own lift is barely 1 LU, a small reduction
  can push it under. Rule 3 keeps the finding to cases in which the
  processors, not the arrangement, removed most of it.
- **Only rises:** a drop after a chorus is the same pair of sections the other
  way round and is not reported again.
- **Against the measurements:** the finding fires on drafts 2, 3 and 4, at
  both choruses. It stays quiet on t3, ferry and lofi. Lofi's `a` → `b` meets
  rules 1 and 2, but its 46% share misses rule 3.

### Report

The profile becomes `maac.analyze.default/6`. The report schema stays
`maac.analysis-report/1`, with additions only:

- a top-level `master_chain` list of the chain's node IDs in signal order,
  omitted when empty;
- `dynamics` objects on the processors' sources, as above.

The human output adds a line per processor with its reduction per section. A
report on a project with no compressor or limiter is unchanged apart from
the profile identifier.

## Where the reduction comes from

This is [decision 1](#decisions).

**A. The processors report their own gain (recommended).**

- **Exact:** the maximum is the true per-frame maximum. The mean leaves out
  makeup and drive even when they are automated, and it follows a sidechain.
- **Self-contained:** it does not depend on which ports `--source` selects.
- **Cost:** a read-only tap in the render loop that copies one value per
  analysed processor per frame. It is internal to the renderer: plans,
  formats and audio stay as they are, and existing render-hash tests check
  that the audio does not change.

**B. Compare loudness at each processor's input and output.**

- **For:** no renderer change. Most of the data is already in the report,
  since `bus`, `glue` and `master` are analysed sources by default, and the
  means agree with A's within 0.15 dB.
- **Against:** it needs the makeup or drive at every block, so an automated
  makeup or limiter gain must be evaluated from the plan.
- **Against:** it has no true maximum. A 2 ms limiter dip averages away in a
  100 ms block.
- **Against:** it needs each processor's input port analysed.

The recommendation is A.

## Rejected alternatives

- **Comparing the bus and the output only.** This is what the AI did by
  hand. It shows that a lift was lost but not which processor took it; in
  draft 4, the less obvious one did.
- **A share-only rule**, for example "the chain takes 60% or more of a lift
  of 1 LU or more". It separates these songs too (52% at most on an
  approved song, 71% at least on a draft). But it fires when a 6 LU
  build-up keeps 2 LU, which still lifts.
- **Max short-term loudness** for the lift, as the
  [producer guide](../ai-producer-guide.md) now advises. It reads the loudest
  3 s of a section, so it does not add up with the reductions. Integrated
  loudness and the weighted reduction do: the output lift plus *taken* is the
  lift before the chain.
- **A time average of the reduction in dB.** It is simpler to describe, but
  it is off by up to 0.88 dB from the loudness a processor takes.

## Acceptance

**Known-answer fixtures**

- A compressor with a −20 dB threshold, ratio 2, no knee and no attack or
  release, fed a square wave at −30 dBFS and then −10 dBFS: 0 dB, then 5 dB
  of reduction, with the mean and maximum within 0.05 dB.
- The same compressor with 6 dB of makeup reports the same reduction. A
  limiter driven 6 dB into its ceiling by a steady sine reports about 6 dB.
- Two sections into a compressor at ratio 10: `lift_lost` fires. At ratio
  1.5 it does not.
- A compressor on a stem gets a `dynamics` object but is not in the master
  chain. A sidechain compressor's reduction follows its detector.
- With and without the tap, the rendered output is byte-identical.

**Songs**

- Drafts 2, 3 and 4 (outside the repository): `lift_lost` at both choruses.
- t3, ferry and lofi: no `lift_lost`.
- A regression test runs "Morning Ferry" with its glue at ratio 10, over a
  window of the end of verse 1 and the start of chorus 1, to keep debug-build
  time down.
- Analysis time on "Morning Ferry" grows by under 2% in a release build.

**Listening**, if [decision 3](#decisions) asks for it

- Four files of draft 4's first verse and chorus (bars 5–20, 34 s), with the
  glue at a −30 dB threshold and ratios 1, 1.5, 3 and 10. The limiter is kept
  out of the way (8 dB lower into the limiter, no drive). The files are
  matched to the same loudness.
- Their output lifts measure 1.78, 1.11, 0.41 and −0.08 LU. The lift before
  the chain is 1.78 LU in each. At the 1 LU starting value, `lift_lost`
  fires on the last two.
- The owner says from which file the chorus stops lifting. The limit goes
  between the last file that lifts and the first that does not.

**Documentation**

- [analyze.md](../analyze.md) gains a `default/6` section.
- In the [producer guide](../ai-producer-guide.md), the "Master" advice and
  the pitfall "The chorus does not lift" name `lift_lost` and the `dynamics`
  objects instead of a hand comparison.
- The T3 trial's F20 row and item 2, [capabilities.md](../capabilities.md),
  the CHANGELOG and the [language plan](language-specification-plan.md)'s AI
  production tooling row are updated.

## Not proposed

- **A finding for heavy reduction alone**, such as a limiter that averages
  3 dB. How hard to compress is a genre choice, and no trial suffered from it.
  The numbers are in the report.
- **Lift through processors outside the master chain.** A stem compressor
  changes one part's level, not the mix's lift.
- **Pumping and release-time checks.** These need a model of how reduction
  sounds over time, and no trial has asked for one.
- **Saturation**, such as `synth.drive/1`. It shapes the waveform rather than
  applying a gain, and [F22](../reports/ai-production-trial-t3.md#friction)
  already gave it a level reference.
- **Automatic fixes.** `analyze` never changes the source.

## Cost and risk

- **Cost: small to medium.** The work is the render tap, the weighted means
  and maxima, the master-chain walk, the finding, the fixtures and the
  documentation.
- **Main risk: the limit.** Three drafts and three approved songs are few data
  points, and nobody has judged a lift by ear yet. The listening round is the
  cheapest way to ground it. Lofi's `a` → `b` sits near both limits: 0.74 LU
  left and 46% taken. A limit between 0.74 and 1 LU would turn that case on
  rule 3 alone.
- **Second risk: the tap changes the audio.** It only reads state after each
  frame. The byte-identical render fixture guards it.

## Decisions

The owner accepted every recommendation on 2026-10-13:

1. **Where the reduction comes from:** the processors report their own gain
   during the analysis render. *Recommended* over comparing loudness at each
   processor's input and output, which loses the true maximum and needs
   automated makeup or drive evaluated separately. See
   [where the reduction comes from](#where-the-reduction-comes-from).
2. **Finding:** `lift_lost`, using the
   [three rules above](#finding-lift_lost). *Recommended* over a share-only
   rule, which fires on large build-ups that still lift, and over reporting
   the numbers with no finding.
3. **Limit:** 1 LU as a starting value, confirmed by a four-file listening
   round on draft 4's first verse and chorus. *Recommended:* nobody has judged
   a lift by ear yet. Alternatively, the limit can rest on the measurements
   alone.
4. **Severity:** `warning`. *Recommended* over `info`. The arrangement's lift
   shows what the author meant, and in T3 the lost lift cost three iterations.
5. **Report:** profile `maac.analyze.default/6` and schema
   `maac.analysis-report/1`, with additions only. *Recommended*, as for
   `default/3` through `default/5`.
