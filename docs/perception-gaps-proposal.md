# Perception gaps: proposal

**Status:** accepted 2026-10-11; the owner accepted every recommendation.
Part A is implemented as profile `maac.analyze.default/3`; its
[results](#results-part-a) record two rules that changed during tuning. Part B
is not started. This is item 6 of [what to build next](ai-production-trial.md#what-to-build-next-in-order),
the last open item from the T2 production trial.

## Why

An AI producer has no ears; [`maac analyze`](analyze.md) is how it listens.
Two kinds of problem still pass through it unheard.

**Steady noise (F13).** In the [T2 trial](ai-production-trial.md), "Late
Window" had a vinyl-hiss chain about 27 dB under the music. Every number looked
fine, and no finding fired. The owner heard "a constant sssss in the
background". The hiss entered the mix bus before the glue compressor and the
limiter's +9.8 dB of gain, so in quiet passages it rose with everything else.
Steady broadband noise is far more noticeable than its loudness suggests. The
[producer guide](ai-producer-guide.md#6-pitfalls-from-the-trial) can only say
"Not measured yet; avoid steady noise".

**Score–audio agreement.** `analyze` measures audio and reads the score, but
never compares them. It cannot say whether a written note can be heard, or
whether it sounds at the written pitch. [analyze.md](analyze.md#deferred-to-later-versions)
deferred these checks. Errors they would catch:

- a note that produces no output, for example one played while an
  `automation` holds its instrument's level at zero. Notes the language can
  rule out, such as a sample note outside every zone, are already refused with
  `E_RANGE`;
- a melody note buried under another part in its own register;
- a pitched sample with the wrong `root`, which puts every note off by the same
  interval; or a detuned or mistransposed wavetable.

A listener hears all of these at once. Until now an AI has had to guess.

## What MaaC can do that a generic analyzer cannot

A generic analyzer sees one mixed file. `maac analyze` knows the graph, each
stem's mix point, and every note's source address, target node, pitch and
frames. So it can say:

> In region `outro`, node `vinyl` is steady (its level varies by 1.8 LU) and
> makes up 42% of the 8 kHz band at its mix point `bus` in the quietest
> passages, where the master measures −31 LUFS.

> Note `melody/bar7/n3` (A4, bar 7) sounds 98 cents sharp.

Both point the AI at the source line to edit.

## Proposed contract

This is tooling only. Nothing changes in the grammar, source kinds, plans or
delivery formats. The work comes in two parts, each with its own profile
version (see [decision 1](#decisions)).

### Part A: noise floor (F13)

**New measurements,** for every source and section. They come from the
100 ms blocks that the meter already keeps, so they cost almost nothing:

| Field | Meaning |
| --- | --- |
| `floor_lufs` | Loudness of the quietest 10% of the section's blocks. Only blocks above −70 LUFS count, as with the existing loudness floor |
| `block_range_lu` | The 95th percentile of block loudness minus the 10th. A steady source has a small range |
| `floor_bands_db` | Octave-band energy of those quietest blocks, relative to their total. This is the spectrum of the floor |

**New finding `exposed_noise`** (warning). It fires for a stem in a section
when all three conditions hold:

1. **The stem is steady.** `block_range_lu` is at most 6 LU, and the stem is
   active in at least 90% of the section's blocks.
2. **The stem is what fills a band.** Take the master's quietest blocks (its
   `floor_lufs` blocks). In those blocks, the stem supplies at least 10% (−10
   dB) of the energy at its mix point in some octave band. That band must be
   within 30 dB of the mix point's strongest band.
3. **The passage can be heard.** The master's `floor_lufs` is above −60 LUFS.

The share is measured at the stem's **mix point**, the bus it feeds, and not at
the master. A mix point is a plain sum, so the share there is close to exact.
Compressors and gain after the bus change the share at the master, while
loudness at the master is what the listener hears. Condition 3 uses the
master, so a compressor lifting the hiss in a quiet passage shows up as it
did in the trial.

The finding gives the stem, the section, the band, the share, the stem's
`block_range_lu` and the first floor block's position.

### Part B: score–audio agreement

**What is measured.** Every note and hit event whose target node is an
analysed source gets a **note window**. It runs from the event's `on_frame` to
the earlier of its `off_frame` and 250 ms later, cut to the analysed range.
While rendering, the meter adds up energy in these windows for two things:

- the target node's output;
- the master, in the octave band that contains the note's written pitch (for a
  hit, the K-weighted broadband level).

Pitch is estimated only for notes on a **monophonic stretch**: no other note
on the same node overlaps the window, and the note has no pitch expression.
The method is a time-domain difference function (YIN). It runs on the window
after its first 30 ms, searches lags from half to twice the written period,
and needs at least four periods. It is deterministic binary64 on one thread,
with no FFT, and reports the deviation in cents and an aperiodicity value. A
window that is not clearly periodic is reported as `unpitched` and gets no
pitch finding. Drums, bells and noise fall here.

**New findings:**

| Kind | Severity | Fires when | Notes |
| --- | --- | --- | --- |
| `silent_note` | warning | The target's output over the note window is below −70 dB K-weighted | Names the event's source address. Catches notes that produce nothing |
| `buried_note` | info | The target's energy in the note's band is at least 20 dB under the master's in that band | One finding per part per section, with a count and the first note's address, so a buried part gives one line, not fifty |
| `pitch_mismatch` | warning | The measured pitch differs from the written pitch by more than 50 cents | Gives the cents. An octave error reads ±1200 |

**Per-part summary.** Each source that receives events gets a `notes` object
for the whole piece and each section: notes measured, silent, buried, pitched
and unpitched, and the median and largest pitch deviation in cents. A full
list of every note's numbers is written only with `--notes` (see
[decision 3](#decisions)).

**A known limit.** A node playing a chord sums its notes, so a quiet note
inside a loud chord on the same node is not reported as buried. The window
measures the part, not the single note. The documentation says so.

### Common rules

- **Windows.** With `--window`, notes count when their onset is inside the
  window, and the noise measurements cover the window's blocks only, like every
  other measurement.
- **Sources.** Checks need their sources. If `--source` drops a stem's mix
  point or a note's target, that check is skipped and the report says so. It is
  never guessed.
- **Determinism.** Same plan and environment, byte-identical JSON. Thresholds
  come from the named profile, never from taste.

## Rejected alternatives

- **A psychoacoustic loudness and masking model** (ISO 532-1 or a Zwicker
  model). It is more faithful, but hard to verify, slow, and its numbers do
  not point to a source line. The proposed checks are simple measurements
  against stated limits, as [analyze.md](analyze.md#findings) requires.
- **Noise detection on the master only.** No stems are needed, but the analyzer
  cannot tell intended content from noise. A quiet sustained pad would read as
  a noise floor. Using steady stems gives far fewer false alarms, and it names
  the node to fix.
- **FFT-peak pitch estimation.** At the window lengths that short notes allow,
  frequency resolution is too coarse to measure 50 cents on low notes. The
  difference function measures period directly.

## Acceptance

Thresholds above are starting values. They are tuned on these known-answer
fixtures before the profile is fixed:

**Part A**

- A sine pad with a two-bar gap, plus `core.noise/1` through a high-pass EQ at
  −30 dB into the same bus, then a compressor. `exposed_noise` fires in the gap
  section only. With the noise at −80 dB, nothing fires.
- **"Late Window" with its hiss restored.** The trial's hiss chain was removed
  before the track was committed, so a test fixture rebuilds it: noise →
  high-pass → gain into `bus`, about 27 dB under the music, as in the trial.
  `exposed_noise` fires, at least in `intro` and `outro`. The committed "Late
  Window" gives no `exposed_noise`.
- The stereo showcase and the other examples give no `exposed_noise`, or each
  one is explained in the results.

**Part B**

- A note played while an `automation` holds its instrument's level at zero
  gives `silent_note`, naming the event's address. The same note after the
  automation rises gives none. The exact fixture is chosen during
  implementation; it must be valid source that `maac check` accepts.
- A quiet melody under a loud pad in the same octave gives one `buried_note`
  for the melody part. Raising the melody by 20 dB clears it.
- A sample declared with its `root` one semitone off gives `pitch_mismatch` of
  about +100 cents on every monophonic note. A 30-cent detune is measured and
  reported in the summary, with no finding. An octave error reads about ±1200.
- Kit hits get no pitch check. A chord on one node is skipped as not
  monophonic, and a bell or noise voice reads `unpitched`.
- "Late Window" and the showcase give no `silent_note` or `pitch_mismatch`,
  or each one is a real error that is fixed.

**Both parts**

- Byte-identical reports on a rerun. Window results equal the matching part of
  a whole-piece report, except for the noted 100 ms block grid edge.
- Analysis time on "Late Window" grows by no more than 25% in a release
  build.
- [analyze.md](analyze.md) gains a profile section. The
  [producer guide](ai-producer-guide.md) pitfalls table names `exposed_noise`
  instead of "Not measured yet". The trial's F13 row and item 6, the
  [language plan](language-specification-plan.md)'s AI production tooling row,
  the CHANGELOG and [capabilities.md](capabilities.md) are updated.

## Not proposed

- **Onset alignment** between written and sounding attacks. It was also on
  the deferred list, but an attack detector for every instrument family is a
  separate problem. `groove_mismatch` already covers the error the trial hit.
- **Hum at 50 or 60 Hz.** Octave bands are too coarse to separate hum from
  bass, and no MaaC source creates hum.
- **Pitch for chords** or for notes with pitch expressions.
- **Reference profiles** and **masking between named pairs**. These stay
  deferred.
- **Automatic fixes.** `analyze` never changes the source.

## Cost and risk

- **Cost:** small for part A, which reuses the existing blocks. Medium for
  part B: note windows, the difference function, the summaries and the
  fixtures. Pitch work runs only on monophonic notes, and its search is limited
  to the written pitch's octave on each side, which keeps a 600-note track
  under about one second of extra release-build time.
- **Main risk:** thresholds that fire too often or too rarely. As with
  `default/2`, they live in a versioned profile and can be tuned without
  changing the measurements.
- **Second risk:** an `exposed_noise` that fires on intended steady content,
  such as a drone or a field recording. The finding is a warning, not an error,
  and its message says the source is steady, so a deliberate choice is easy to
  keep.

## Decisions

The owner accepted every recommendation on 2026-10-11.

1. **Delivery.** Part A first, as profile `maac.analyze.default/3`. Part B
   follows as `default/4`. *Recommended:* part A is small and closes F13 on its
   own, and each profile change stays traceable to one set of findings.
2. **Noise detection.** Steady stems with their share measured at the mix
   point, and audibility measured at the master, as above. *Recommended* over
   detection on the master alone; see [rejected alternatives](#rejected-alternatives).
3. **Per-note output.** A per-part summary and findings by default, and the
   full per-note list only with `--notes`. *Recommended:* a 600-note track
   would otherwise bury the report an agent reads first.
4. **Report schema.** Keep `maac.analysis-report/1` and add the new fields,
   plus an optional `event` address on findings. Existing fields do not change
   meaning, and JSON readers ignore keys they do not know. *Recommended* over
   moving to `/2`, which would make every consumer change for an addition.
5. **Pitch method.** A time-domain difference function (YIN) within an octave
   of the written pitch. *Recommended* over FFT peaks, for resolution on short,
   low notes.

## Results: part A

Implemented as profile `maac.analyze.default/3`, with no change to the report
schema. `src/analyze/meter.rs` derives `floor_lufs`, `block_range_lu` and
`floor_bands_db` from the existing blocks; `src/analyze/mod.rs` adds
`exposed_noise`. Tests in [`tests/analyze.rs`](../tests/analyze.rs) cover the
measurements, a noise left alone in a gap, faint noise, a held note, findings
per region, and "Late Window" with the trial's hiss restored.

Tuning on "Late Window" changed two rules from the text above:

1. **A steady stem must also be one that no note drives.** As proposed, the
   released track gave a false alarm. Its pad is as steady as noise (2.66 LU
   in region `b`) and made up 29% of the 125 Hz band at `bus`. A stem now
   counts only when no node that receives score events lies at or upstream of
   it. Notes make music the author wrote, and its steadiness is a choice. The
   hiss runs on its own. A noise played by a held note is therefore not
   reported.
2. **Audibility is the noise's own level at the output.** Rule 3 above asked
   for the output's floor to be above −60 LUFS. That passed for a hiss 44 dB
   under the music, because the intro and outro have near-silent moments in
   which any noise is the whole band. The rule now estimates the stem's own
   loudness at the output in the floor blocks: its K-weighted share of the mix
   point, times the output's energy there, so later gain and compression still
   count. It must be above −50 LUFS, about 35 LU under a −14.7 LUFS master.

"Steady" also uses the −70 LUFS gate instead of the −60 dBFS activity
threshold, so a quiet noise that compression later lifts still counts.

Hiss added to "Late Window" (noise → 3 kHz high-pass → gain → `bus`):

| Gain | Under the bus | At the output, quiet passages | `exposed_noise` |
| --- | --- | --- | --- |
| none (released track) | — | — | none |
| 0.01 | 13 dB | about −28 LUFS | every region |
| 0.003 (about the trial's level) | 24 dB | about −37 LUFS | every region |
| 0.001 | 33 dB | about −46 LUFS | `intro`, `a`, `outro` |
| 0.0003 | 44 dB | about −55 LUFS | none |
| 0.0001 | 57 dB | below −70 LUFS | none |

Every other example, including the stereo showcase, gives no
`exposed_noise`. The regression test analyses bars 1–2 of "Late Window" with
and without the hiss, to keep its debug-build time near 30 seconds.
