# Measuring the pump: proposal

**Status:** accepted 2026-10-15, when the owner took both recommended
[decisions](#decisions), and implemented; [Results](#results) records the
evidence. This is item 3 of
[what to build next](../reports/ai-production-trial-t5.md#what-to-build-next-in-order)
in the T5 trial: F33, a pump that `maac analyze` could not tell was unheard.
It is a tool change, analysis profile `maac.analyze.default/9`. The
language, the plan formats and every other measure are unchanged.

## Why

"Afterlight" pumps: an `fx.compressor/1` keyed by the kick through its
`sidechain` input ducks the music bus on every beat. In version 1 the
analyzer's `dynamics` said the pump took 4.3 dB on average from the drops,
up to 13.9 dB, and the owner said the drop "did not feel pumped".

`dynamics` reports what a compressor takes from its own input, not what the
mix does. The AI found the cause only with a script outside the analyzer.
The script measured the master above 300 Hz and folded it onto the beat.
From its loudest to its quietest point the level moved only 6.1 dB per
beat. Deeper pump settings barely changed that (6.8 and 6.9 dB), because
the lead and the drums carried the drop and bypassed the pump, so they
covered the chords that went through it. With the lead through the pump
the mix moved 14.9 and 17.9 dB, and the owner chose the first: "With the
pumping in, it's so exciting and good."

Nothing in the report could have told the AI before the listener did.

## Proposed contract

### Beat dip

Every source's measures, over the whole piece and in each section, gain
`beat_dip_db`: how far the level above 300 Hz dips over the beat.

- **Above 300 Hz.** The source is summed to mono and high-passed at 300 Hz
  by two one-pole stages, as in the T5 script. This leaves out the kick and
  the sub that key a pump: they peak on the beat and fill the dip the pump
  makes.
- **Folded on the beat.** Every whole quarter note of the score in the
  analysed window is cut into 24 equal parts by frame. A span takes the mean
  power of each part across its whole quarter notes. The parts follow the
  tempo map, so a tempo change moves them with the beat.
- **The dip.** The quietest of the 24 parts, under their upper quartile
  (the nearest-rank 75th percentile, the 18th part from the quietest), in
  dB. The upper quartile is the level the music returns to between dips.
  A drum hit fills only a few parts of the beat, so it does not raise that
  level, while a pump's dip lasts long enough to reach the quietest part.
- **When there is none.** It is absent under four whole quarter notes, and
  when the source's mean square above 300 Hz is at or under −60 dBFS. A part
  that is silent counts as 60 dB under the quartile, so the dip is at most
  60 dB.

### Finding `pump_unheard`

A warning for each compressor keyed through its `sidechain` input, in each
section where:

- its `dynamics.reduction_db` is at least 3 dB, so it pumps there, and
- the output's `beat_dip_db` is under 8 dB.

The finding names the compressor's output port and the section. Its value
is the output's dip and its threshold 8 dB. Its message adds the dip of the
compressor's own output. A large one means the pump is covered; a small
one means it ducks too little:

```text
warning pump_unheard pump:out [drop1, bar 33]: the keyed compressor takes 8.11 dB, but the output's level above 300 Hz dips only 5.28 dB over the beat; `pump:out` alone dips 13.96 dB
```

- **8 dB** lies between the pumps the owner did not hear and those they
  did. In "Afterlight"'s drops, the unheard pumps dip 4.7 to 5.8 dB and the
  heard ones 12.1 to 13.0 dB. No section of the trials' songs without a pump
  dips more than 5.6 dB. See [Measurements](#measurements).
- **3 dB** separates a pump from ducking that only makes room. "Afterlight"'s
  pump takes 7.4 to 9.4 dB in the sections it pumps. The bass ducking in
  [`examples/production.maac`](../../examples/production.maac) takes
  0.95 dB, and a warning there would be noise.

### Report

The human output's line for each section of the project output ends with
`beat dip X dB`. The report schema gains the one field; its identifier is
unchanged, as when `default/6` added `dynamics`.

## Measurements

### What separates a pump from a beat

Before the analyzer changed, a script folded eight excerpts onto the beat
above 300 Hz, as above, and compared two measures. The excerpts are three
pumps the owner heard; two with the lead after the pump, as in the version
in which the owner heard none; and three songs without a pump. They are the
first eight bars of "Afterlight"'s first drop, and the first verse or the
second A section of the others.

| Excerpt | Owner | Loudest − quietest (dB) | Upper quartile − quietest (dB) |
| --- | --- | ---: | ---: |
| "Afterlight", approved | pump approved | 13.9 | **11.6** |
| T5 pump round 1, lead through the pump | "better", "so exciting" | 14.9 | **11.6** |
| T5 pump round 2, lead through the pump | "not bad either" | 18.0 | **11.7** |
| "Afterlight", lead after the pump | as version 1: no pump | 5.9 | **5.3** |
| T5 pump round 1, lead after the pump | not played | 6.9 | **5.9** |
| "Morning Ferry", first verse | no pump | 11.8 | **5.2** |
| "Lantern Street", first verse | no pump | 9.2 | **4.7** |
| "Late Window", second A section | no pump | 11.0 | **3.7** |

The T5 script's measure, loudest over quietest, cannot separate them: the
songs without a pump reach 11.8 dB, more than the unheard pumps and close to
the approved one. Their drums make it: the hits stand 6 to 9 dB over the
middle of the beat for a part or two, and a covered pump's dip is
shallower than that. The upper quartile leaves the drum hits out, and the
two groups lie 5.7 dB apart.

### Every section of "Afterlight"

Full renders, the output's dip and what the pump takes. "Lead after the
pump" changes only the two connections that send the lead to the music bus,
sending it to the final bus instead, as in version 1.

The analyzer's `beat_dip_db` for the output, with what the pump takes, in
every region:

| Region | Approved: pump takes (dB) | Approved: dip (dB) | Lead after: pump takes (dB) | Lead after: dip (dB) |
| --- | ---: | ---: | ---: | ---: |
| intro | 9.4 | 13.4 | 9.4 | 13.4 |
| groove | 7.4 | 12.4 | 7.4 | 12.4 |
| breakdown | 0.1 | 2.6 | 0.1 | 2.6 |
| build | 0.0 | 6.4 | 0.0 | 6.4 |
| drop1 | 8.4 | **12.2** | 8.1 | **5.3** |
| interlude | 0.1 | 2.1 | 0.1 | 2.1 |
| build2 | 0.0 | 5.9 | 0.0 | 5.9 |
| drop2 | 8.5 | **12.2** | 8.1 | **4.7** |
| outro | 7.6 | 11.9 | 7.6 | 12.0 |

The lead also plays in the interlude, the second build and the outro, but
it changes the dip only in the drops: elsewhere the pump takes nothing, or
the lead is quiet. The pump's own output dips 13.7 and 13.9 dB in the first drop of
the two versions; at the output the lead after the pump fills that dip.
`pump_unheard` fires for both drops of the second version and for nothing
in the approved one. The T5 pump round's songs agree: the first setting
with the lead after the pump dips 5.8 and 5.3 dB in the drops and is
reported; with the lead through it, 12.1 dB, and the second setting
13.0 and 12.9 dB, are not.

### Songs without a pump

No section of the other example songs comes near 8 dB, and none has a
keyed compressor:

| Song | Sections | Deepest dip (dB) |
| --- | ---: | --- |
| "Morning Ferry" | 6 | 5.2, first verse |
| "Lantern Street" | 8 | 5.6, intro |
| "Late Window" | 5 | 3.7, second A section |
| [`evening-window.maac`](../../examples/evening-window.maac) | 4 | 0.6 |

The bass ducking in [`examples/production.maac`](../../examples/production.maac)
takes 0.95 dB and the output dips 1.9 dB; it raises no finding.

## Rejected alternatives

- **The T5 script's loudest over quietest.** Drum hits raise it as much as a
  pump does; see [above](#what-separates-a-pump-from-a-beat).
- **The median as the reference.** It separates the excerpts as well, but a
  pump whose dip lasts over half the beat pulls the median into the dip.
- **Folding on the key's onsets.** It would follow a kick that leaves out
  beats, but needs the key's onsets from the audio. The quarter-note grid is
  exact from the tempo map, and every pump in the trials follows it.
- **Full-band or K-weighted level.** The kick at the top of every beat fills
  the pump's dip.
- **The compressor's folded gain reduction.** It says how deep the pump
  ducks, which `dynamics` already shows, not whether the mix dips: the T5
  failure.
- **A finding without a keyed compressor.** Many songs barely dip over the
  beat; only a keyed compressor says a pump is meant.
- **A `--pump` flag, like `--lead`.** The keyed compressor states the intent
  already, and the 3 dB floor leaves out ducking that only makes room.

## Limits

- **Sparse sections dip anyway.** A section of short notes and rests, or one
  that drums carry alone, dips deep without a pump: "Afterlight"'s intro,
  its music low-passed, dips 13.4 dB. `pump_unheard` judges only a dip
  under 8 dB, so it does not misfire there, but a large dip shows a pump only
  where sustained parts carry the section.
- **Quarter notes.** A pump on half notes, or a kick that leaves out beats,
  folds ducked beats with others, and reads shallower.
- **One listener and one track.** The threshold comes from T5 alone.

## Acceptance

1. A tone that drops 10 dB for the second half of every quarter note dips
   10 dB, at a steady tempo and across a tempo change; the tone alone dips
   0 dB.
2. A sub that drops 10 dB under a steady tone leaves the output's dip under
   0.5 dB.
3. Under four whole quarter notes, or for a silent source, there is no dip.
4. A keyed compressor whose ducked pad a steady lead covers is reported in
   that section, as a warning with the output's dip, and not in the section
   without the lead. An unkeyed glue compressor taking over 3 dB is not
   reported.
5. The human output shows each section's dip.
6. The first eight bars of "Afterlight"'s first drop dip over 10 dB with no
   `pump_unheard`, and under 7 dB with the lead after the pump, where
   `pump:out` is reported.
7. [analyze.md](../analyze.md), the [AI producer guide](../ai-producer-guide.md),
   the capability matrix and the T5 report describe the change.

## Cost and risk

- **Cost:** small. Per source and frame, two one-pole filters and an add.
- **Risk:** a warning for a keyed compressor meant only to make room that
  takes over 3 dB. Its message says what it measured, so the author can
  decide it is intended.

## Decisions

1. **Severity.** A warning (recommended), like `lead_under`, so the AI fixes
   it or decides it is intended; or `info`, which an AI may skip.
2. **Threshold.** 8 dB (recommended), midway between the unheard pumps and
   the heard ones; or 10 dB, nearer the heard ones.

The owner chose a warning and 8 dB.

## Results

- **Contract.** [`src/analyze/beat.rs`](../../src/analyze/beat.rs) folds
  every analysed source onto the score's whole quarter notes during the one
  render; `beat_dip_db` is in each source's `whole` and section measures,
  and `pump_unheard` is in profile `maac.analyze.default/9`. The human
  output's section lines end with `beat dip`.
- **Tests.** [`tests/analyze_pump.rs`](../../tests/analyze_pump.rs) checks
  acceptance items 1 to 6 on synthesized fixtures and on "Afterlight": a
  tone gated 10 dB on the off-half of each beat dips 10 dB at a steady
  tempo and across a change from 120 to 90 bpm; a gated sub under a steady
  tone leaves the output under 0.5 dB; a covered pump is reported in its
  section only, and a glue compressor taking over 3 dB is not.
- **"Afterlight".** The first eight bars of the first drop, with two seconds
  of preroll, dip 11.51 dB as approved, with no finding, and 5.28 dB with
  the lead after the pump, where `pump:out` is reported, as in the example
  above. The script measured the same excerpts at 11.6 and 5.3 dB. Every
  region is in the [table above](#every-section-of-afterlight).
- **Documents.** [analyze.md](../analyze.md) describes the profile; the
  [AI producer guide](../ai-producer-guide.md) checks `beat_dip_db` when
  pumping and names `pump_unheard` for "It doesn't feel pumped"; the
  capability matrix, the changelog and the T5 report record it.
