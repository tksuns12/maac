# Lead part: proposal

**Status:** proposed 2026-10-12, awaiting the owner's [decisions](#decisions).
This is the first half of item 2 of
[what to build next](../reports/ai-production-trial-t3.md#what-to-build-next-in-order)
in the T3 trial: F21, a lead role in the analysis. The second half, gain
reduction per section (F20), measures something else and gets its own
proposal.

## Why

In both production trials the melody sank under its accompaniment, and
[`maac analyze`](../analyze.md) said nothing:

- **T2, "Late Window":** the first mix had the bell melody 6 dB under the
  keys.
- **T3, "Morning Ferry":** the lead guitar read 11 dB under the picking in
  the verses, because a lower amp drive also lowered the amp's output (F22).

Both times the AI found the problem by comparing stem loudness per region by
hand. No finding fired (F21).

`buried_note` is a floor for every part. It fires when a part holds −20 dB or
less of its own strongest band at its mix point. A melody can sit well above
that floor and still not lead. In the T3 case, measured again below, the
median lead note held −12 dB of its band in the verses and −8 dB in the
choruses: under the accompaniment, but nowhere near −20 dB.

The analyzer cannot do better on its own, because it does not know which part
is meant to lead. A pad 11 dB under the picking is fine; a melody is not. The
[producer guide](../ai-producer-guide.md) works around this with advice to
"aim for the lead within about 2 dB of the main accompaniment", checked by
hand from per-stem loudness.

## Measurements behind this proposal

Before proposing a rule, the existing per-note measurement was read from
three versions of "Morning Ferry" and from "Late Window", all with profile
`maac.analyze.default/4`:

| Version | What it is |
| --- | --- |
| ferry | The released song, on the `std/studio/1.1.0` guitar |
| t3 | The song as delivered in the trial (commit `55c2ae8`), which the owner approved: "Good. It's good." |
| t3 buried | t3 without its `lead_level` automation, which restores the trial's problem |
| lofi | "Late Window", with the nylon guitar as the lead |

`--notes` gives each note a `share_db`. This is the part's share of its mix
point, `bus` in both songs, in the octave band where the part is strongest
over the note. A share *s* converts to the lead's level against the rest of
the mix point in that band: 10·log10(*s* / (1 − *s*)), with *s* as a power
ratio. This proposal calls that the lead's **margin**.

| Version | Regions | Lead notes | Median margin | Quietest note | Notes at −6 dB or less |
| --- | --- | --- | --- | --- | --- |
| ferry | verses | 35 each | −1.6 dB | −6.0 dB | 0 |
| ferry | choruses | 31 each | −1.5 dB | −7.5 dB | 1 |
| t3 | verses | 35 each | −1.0 dB | −5.3 dB | 0 |
| t3 | choruses | 31 each | −1.6 dB | −9.0 dB | 1 |
| t3 buried | verses | 35 each | −11.3 and −11.4 dB | −16.1 dB | 35 |
| t3 buried | choruses | 31 each | −7.7 dB | −15.6 dB | 22 |
| lofi | `b` | 24 | +0.8 dB | −6.1 dB | 1 |
| lofi | `a2` | 8 | +3.4 dB | +1.0 dB | 0 |

- **The approved mixes** put the median lead note between −1.6 and +3.4 dB
  against the rest of its band.
- **The buried version** puts it at −7.7 to −11.4 dB. The verse figure
  agrees with the trial's "11 dB under the picking". `default/4` gave this
  version no warning at all.
- **Section loudness misleads for a part that rests.** The released lead
  plays no note in the intro, yet reads −23.8 LUFS there. The intro's last
  100 ms block starts before the verse and holds the first lead note's
  attack, the same block-edge effect as F24. The lead check therefore
  measures the lead's notes, not its section loudness.

## Proposed contract

This is tooling only, if [decision 1](#decisions) goes as recommended.
Nothing changes in the grammar, source kinds, plans or delivery formats.

### Naming the lead

```sh
maac analyze main.maac --section regions --lead lead:out
```

`--lead NODE:PORT` names a lead part, in the form `--source` already uses. It
may be repeated, for a song whose lead passes from one part to another or in
which two parts lead together. A lead must meet three rules:

1. **It is a part, not the mix.** It is an audio output port that reaches the
   project output and is not the output itself.
2. **It has a mix point.** The lead is read at its mix point: the first mix
   point downstream, as for `buried_note`. When it feeds several at the same
   distance, the analyzer takes the one nearer the output. "Morning Ferry"'s
   lead feeds both `bus` and the reverb `send`; the rule takes `bus`.
3. **It plays notes.** Some node at or upstream of the port receives notes.
   Those notes are the lead's notes.

A port that breaks a rule is `E_USAGE`, with a message naming the port and
the rule; an unknown port is `E_REFERENCE`, as for `--source`. `--lead` adds
the port and its mix point to the analysed sources, so it also works with
`--source`. With `--window`, a lead note counts when its onset is inside the
window, as for the other note checks.

### Measurement

Each lead note uses the note window of the `default/4` checks: from onset to
release, at most 2 s. Over the window the analyzer reads the energy of the
lead port and of its mix point in each octave band, and takes the lead's
strongest band. The note's margin is

> lead energy ÷ (mix point energy − the energy of every lead feeding that mix
> point), in dB.

- **Other leads are subtracted.** Two leads playing together are each
  compared with the accompaniment, not with each other.
- **The subtraction treats the parts as uncorrelated.** This holds for
  different instruments at a plain sum. A lead's own reverb return counts as
  accompaniment, which slightly lowers its margin.
- **A clear band:** when the rest of the mix point is silent in that band, the
  note has no margin and counts as clear.

### Finding `lead_under`

A **warning**, one per lead and section. It fires when the section holds at
least 4 of the lead's notes and their median margin is −6 dB or less:

> In `verse1`, lead `lead:out` sits 11.3 dB under the rest of `bus` in its
> strongest band (median of 35 notes; limit −6 dB). The quietest note is
> `melody_verse1/0/c3` at −16.1 dB.

Its `value` is the median margin, `threshold` is −6, and `event` and `at` name
the quietest note.

- **The median:** a few soft passing notes are expression. A section in which
  half the melody sits under the accompaniment is a balance problem.
- **At least 4 notes:** a pickup note just over a section edge should not
  judge a whole section. `groove_mismatch` uses the same minimum.
- **−6 dB:** on the measurements above, this fires on every buried section
  (−7.7 dB at best) and on no approved one (−1.6 dB at worst). That leaves
  about 4 dB of room on the approved side and 1.7 dB on the buried side. It is
  a starting value; see [decision 3](#decisions).
- **With `buried_note`:** both can fire on the same lead. `buried_note` says a
  note can hardly be heard; `lead_under` says the melody does not lead.

### Report

The profile becomes `maac.analyze.default/5`. The report schema stays
`maac.analysis-report/1`, with additions only, as for `default/3` and
`default/4`:

- a top-level `leads` list of the named ports, omitted when empty;
- for each lead source, a `lead` object in `whole` and in each section, with
  `mix_point`, `notes`, `median_db`, `min_db` and `under` (notes at or below
  the limit);
- with `--notes`, a `lead_db` margin on each lead note.

Without `--lead`, every report is the same as under `default/4` apart from
the profile identifier.

## Where the lead is named

This is [decision 1](#decisions). The T3 record suggested letting the score
mark the lead. Weighed against the specification, a flag does the job at a
fraction of the cost. There are three places for it.

**A. A flag on `maac analyze` (recommended).**

- **No language change.** [§19](../../MaaC-1-Specification.md#19-regions-and-non-rendering-information)
  already says "analysis annotations are not sound". The
  [design principles](design-principles.md#evaluate-features-at-the-appropriate-boundary)
  ask a proposal to "explain why existing library, extension, or tool
  contracts cannot meet the need" before the language grows. A flag meets the
  need.
- **It matches the analyzer's other choices:** `--section`, `--window` and
  `--source` are flags too.
- **Cost: one argument per run.** The T3 producer ran the same `analyze`
  command on every iteration, and the producer guide's commands will include
  `--lead`.
- **Cost: the intent is not in the source.** A later session that reads only
  the source does not learn which part leads. A comment beside the lead node
  covers that until a durable form is needed.

**B. A field in the score**, for example
`track melody { target = &lead:events; role = lead; }`.

- **For:** the intent lives with the music, and every tool and later session
  sees it.
- **Against:** it is a core change. §4 says "All objects may carry the
  optional string field `label`. No other implicit fields exist." A role field
  needs an amendment to §10, a hash role like `label`'s under §20.2, and
  editing-protocol and conformance coverage. It also tells the renderer
  nothing, and §1.1 keeps fields without acoustic behavior out of the core.
- **Against:** a track is the wrong grain. The analyzer hears nodes, and a node
  can take several tracks.

**C. A sidecar file** keyed by source IDs, as §19 suggests for UI state.

- **For:** durable, with no language change.
- **Against:** a new file format to version, which does not follow ID renames,
  and nothing in `maac` reads sidecars yet. It is worth it once several
  analysis settings need to persist; then `--lead`, `--section` and the others
  can move into it together.

The recommendation is A, revisiting B or C if a later trial shows a session
losing track of the lead.

## Rejected alternatives

- **Guessing the lead**, for example as the highest part that mostly plays one
  note at a time. Both songs would be guessed right. A counter-melody, a high
  pad line or a bass solo would not, and a wrong guess turns a balance finding
  into noise. The trial record asks for a marked lead.
- **A stricter `buried_note` for every part.** The −6 dB median rule applied
  to every part flags the pads in both songs: "Morning Ferry"'s pad in both
  choruses at −9.5 dB, and "Late Window"'s in `b` at −17.2 dB. An
  accompaniment belongs under the lead; only the lead needs the stricter rule.
- **Broadband loudness against the loudest other part.** This is what the AI
  did by hand, and it also separates these cases: the buried lead sat 12.8 LU
  under the bass and the approved one 1.5 LU. But it compares the lead with a
  bass that does not cover it, and section loudness counts blocks in which the
  lead does not play. The margin compares the lead with what shares its band,
  note by note, and names the note to fix.

## Acceptance

The limit and minimum above are starting values, tuned on these before the
profile is fixed.

**Known-answer fixtures**

- A sine lead and a sine pad in the same octave into one bus, with the lead
  at 0, −3 and −10 dB from the pad. The margins read 0, −3 and −10 dB within
  0.5 dB, and `lead_under` fires for −10 dB only.
- The −10 dB lead against a bass two octaves lower that is 10 dB louder: no
  finding, because the margin is read in the lead's band.
- Two leads in one section: each is compared with the accompaniment without
  the other.
- A section with three lead notes: a `lead` summary, no finding.
- `E_USAGE` for `--lead` on the output, on a port with no mix point, and on a
  port that no note drives; `E_REFERENCE` for an unknown port.

**Songs**

- t3 buried with `--lead lead:out`: `lead_under` in `verse1`, `chorus1`,
  `verse2` and `chorus2`. The regression test analyses a window of one verse,
  to keep debug-build time down, as the `exposed_noise` test does.
- The released "Morning Ferry" with `--lead lead:out`, and "Late Window" with
  `--lead guitar:out`: no `lead_under`.
- Without `--lead`, every example's report is unchanged apart from the profile
  identifier.
- Analysis time on "Morning Ferry" grows by under 5% in a release build.

**Listening**, if [decision 3](#decisions) asks for it

- Four files of "Morning Ferry"'s first verse, with the lead at 0, −3, −6 and
  −9 dB from the released mix. Their median margins will be about −1.6, −4.6,
  −7.6 and −10.6 dB.
- The owner says from which file the guitar stops leading. The limit goes
  between the last file that leads and the first that does not.

**Documentation**

- [analyze.md](../analyze.md) gains a `default/5` section.
- The [producer guide](../ai-producer-guide.md)'s commands pass `--lead`. Its
  "Balance" advice and its "Lead melody gets lost" pitfall name `lead_under`
  instead of a hand comparison of stems.
- The T3 trial's F21 row and item 2, [capabilities.md](../capabilities.md),
  the CHANGELOG and the [language plan](language-specification-plan.md)'s AI
  production tooling row are updated.

## Not proposed

- **Gain reduction per section (F20).** It needs the compressor and limiter
  to report their own gain, not a measurement of their outputs. A separate
  proposal follows.
- **A lead that is too loud.** Neither trial hit it. The `lead` summary shows
  the margin, and a `lead_over` finding can follow if a trial needs one.
- **A lead without notes**, such as an audio clip or a recorded take. Every
  MaaC song so far plays its lead as notes. A block-based margin could cover
  audio later.
- **A lead that shares a node with its accompaniment**, such as one piano
  playing melody and chords, as in [hello.maac](../../examples/hello.maac).
  The analyzer hears nodes, so the melody needs its own node. The
  documentation will say so.
- **Pairwise masking and reference profiles.** These stay deferred.
- **Automatic fixes.** `analyze` never changes the source.

## Cost and risk

- **Cost: small.** The note windows and their band energies already exist.
  The work is the flag and its rules, the mix-point choice, the margin, the
  summary, the finding, the fixtures and the documentation.
- **Main risk: the limit.** Two approved songs and one buried version are few
  data points. The limit lives in a versioned profile and can be tuned without
  changing the measurements. The listening round in decision 3 is the
  cheapest way to ground it.
- **Second risk: a lead read at a sub-bus.** If a lead and its harmonies meet
  at a vocal bus before the main bus, the margin compares the lead with the
  harmonies only. No MaaC song routes that way yet; the documentation will
  state the limit.

## Decisions

The owner's choices are needed on:

1. **Where the lead is named:** a `--lead` flag on `maac analyze`.
   *Recommended* over a score field, which is a core language change, and a
   sidecar file, which is a new format for one setting. See
   [where the lead is named](#where-the-lead-is-named).
2. **Measure:** each lead note's margin over the rest of its mix point in the
   lead's strongest band, with the median per section. *Recommended* over
   broadband loudness against the loudest other part; see
   [rejected alternatives](#rejected-alternatives).
3. **Limit:** −6 dB as a starting value, confirmed by a four-file listening
   round on the first verse. *Recommended:* the limit is the main risk, and one
   verdict on four short files grounds it in the owner's ears. Alternatively,
   the limit can rest on the measurements alone.
4. **Severity:** `warning`. *Recommended* over `info`, which `buried_note`
   uses. Here the author has named the lead, so a buried lead fails a stated
   intent.
5. **Report:** profile `maac.analyze.default/5` and schema
   `maac.analysis-report/1`, with additions only. *Recommended*, as for
   `default/3` and `default/4`.
