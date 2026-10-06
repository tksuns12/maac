# T2: an AI produces a track end to end

**Date:** 2026-10-04. **Producer:** Claude, an AI model, working in this
repository. **Listener:** the project owner.
**Result:** [`examples/lofi/lofi.maac`](../examples/lofi/lofi.maac), "Late
Window".

MaaC's goal is a framework in which an AI produces a finished track itself.
This trial tested that goal directly. It records what the AI did, what the
owner heard, every point of friction, and what to build next.

## Brief and rules

- **Brief:** 90 bpm lo-fi hip-hop, warm and calm, about 75 seconds. The AI
  chose the brief at the owner's request.
- **Tools:** MaaC source written directly, with no generator script, and only
  `maac check`, `maac analyze`, `maac query-events` and `maac deliver`.
- **Judgment:** the owner listened to each version on a phone and gave the
  verdicts; numbers alone did not decide.

## Outcome

- **Form:** 28 bars, about 75 s plus a 3 s tail. Intro, A, B, A′ and outro are
  marked as regions. The progressions are Fmaj9–Em7–Dm9–Cmaj9 and
  B♭maj7–Am7–Gm9–C9sus.
- **Parts:** electric-piano comping, finger bass, boom-bap drums, a warm pad,
  and a nylon-guitar melody, all on one swing grid.
- **Mix and master:** a glue compressor and a fast peak compressor stand in
  for a limiter. The example has since moved to `fx.limiter/1` (F9).
- **Delivery:** `maac deliver` passed its checks for a 24-bit dithered master
  at -15.58 LUFS integrated and -2.07 dBTP true peak.
- **Owner's final verdict:** the timing "locks", the noise is gone, and the
  guitar is "somewhat better". The std/basic keys, bass, drums and pad still
  sound dated. The example has since moved its keys, bass, drums and pad to
  [std/studio](studio-instruments.md) at the same per-part loudness (F14).

## What happened

| Step | What the AI did | Evidence |
| --- | --- | --- |
| Draft 1 | Wrote 323 lines and 629 notes by hand: nine one-bar chord patterns reused through `use`, one node per drum, a vinyl-hiss chain | `maac check` passed on the first try |
| Iteration 1 | `analyze --section regions` (per source, per region) | Master -24.7 LUFS with an 18 dB crest factor; bell melody 6 dB under the keys; snare weak. Several v1 analyzer problems surfaced (F4–F6) |
| Iterations 2–4 | Rebalanced; added a glue compressor and a fast peak compressor; calculated the master gain from the measured true peak | -16.34 LUFS and -1.43 dBTP, then delivery checks passed |
| Verdict 1 | Owner: "the melody and the beat keep missing each other; some parts only barely line up" | Counting onset positions from `query-events` showed the hats swung while every other part was straight (F12) |
| Fix | Put every part on one swing grid and removed the snare's 15 ms lag | Zero straight off-beats remained |
| Verdict 2 | Owner: "now it locks"; "the instruments sound a bit tacky"; "a constant sssss in the background" | — |
| Iteration 5 | Removed the hiss; replaced the synthesized flute and FM bell with the std/acoustic nylon guitar; darkened keys and drums; re-levelled and re-tuned the peak compressor | -15.58 LUFS, -2.07 dBTP, checks passed |
| Verdict 3 | Owner: "the noise is gone and the guitar is somewhat better" | The remaining std/basic instruments are the timbre ceiling |
| Tooling | Fixed the analyzer problems found here (profile [`maac.analyze.default/2`](analyze.md#profile-maacanalyzedefault2-2026-10-04)) | The track gives no findings; a variant with the old mixed grid gives `groove_mismatch` in every region |

## Friction

`+` marks something that worked well; F-numbers are problems.

**What worked:**

- `maac check` accepted the hand-written draft as written.
- `use` kept a 28-bar keys part to nine one-bar patterns.
- Per-source, per-region loudness made balance decisions concrete.
- Analysing only `--source master:out` made loudness iterations faster.
- `query-events` confirmed the groove problem in seconds.
- `maac deliver` checked the limits and wrote a dithered master.

**Problems:**

| ID | Problem | Status |
| --- | --- | --- |
| F1 | Every note is its own object with an ID; a four-note chord is four objects | Fixed: [`chord` leaves](../MaaC-1-Specification.md#82-chords). "Late Window" now has 21 chords in place of 81 notes |
| F2 | Each synthesized drum needs its own node, track, connection and pattern; there is no drum map | Fixed: [kit instruments](instruments.md#kits) and the `std/basic/1.1.0` `drums` kit. "Late Window" plays kick and hats as hits on one kit node; the snare stays separate because it alone feeds the reverb send |
| F3 | Swing has no construct; swung positions are calculated by hand | Fixed: [`groove`](../MaaC-1-Specification.md#92-grooves). Parts are written straight, and each placement references one groove |
| F4 | `analyze` treated a generator before its gain stage as a mix source | Fixed in `default/2` |
| F5 | `analyze` flagged intentionally panned instruments as stereo imbalance | Fixed in `default/2` |
| F6 | `analyze` printed loudness values such as -1751 LUFS for decaying tails | Fixed in `default/2` |
| F7 | Rendering runs at about real time (65 s for 78 s), so each iteration costs over a minute; `analyze` cannot render one section | Gap: iteration speed |
| F8 | Drums struck on one pitch shared a piano-roll row | Fixed in `default/2` |
| F9, F10, F16 | There is no limiter. Two compressors stand in for one, the crest factor stays high, and the plucked guitar's attacks pushed the true peak over 0 dBTP until the stand-in was re-tuned | Fixed: [`fx.limiter/1`](production.md#6-limiter--fxlimiter1). One limiter replaced the stand-in: -14.73 LUFS at -1.10 dBTP against -15.58 LUFS at -2.07 dBTP |
| F11 | A delivery needs a copied schema file and its hash pin. Its path is project-root relative, while sample paths are source relative | Gap: delivery boilerplate |
| F12 | Mixed swing and straight grids went unnoticed by every tool; the owner's ears caught them | Fixed: `groove_mismatch` |
| F13 | A vinyl hiss 27 dB under the music still read as a constant, annoying "sssss" once the compressors lifted quiet passages. The numbers could not show that steady noise is salient | Gap: perception |
| F14 | The std/basic instruments sound dated, and they limit how good any track can sound | Addressed by [`std/studio/1.0.0`](studio-instruments.md): new building blocks and an electric piano, bass, pad and kit whose timbre follows velocity. "Late Window" now plays them. Accepted on measurements; no one has judged it by ear |
| F15 | `level` has no common loudness scale across libraries; the guitar started 10–14 dB too quiet | Fixed within std/studio: every default puts a reference phrase at velocity 0.7 at −20 LUFS. std/basic and std/acoustic keep their released scales |

## What to build next, in order

1. **Instrument palette (F14).** Production quality cannot exceed instrument
   quality, and this was the owner's lasting complaint. Addressed on
   2026-10-06 by the synthesized route below; see the
   [palette proposal](instrument-palette-proposal.md). Candidates:
   - more modelled instruments like std/acoustic, for example a modelled
     electric piano, upright and electric bass, and drum kits;
   - sampled instruments using the existing pitched sampler, which needs
     recorded or carefully synthesized content and a licence position;
   - character processors (saturation, tape wow and flutter, chorus) for genre
     colour.
2. **A true-peak limiter (F9, F10, F16).** Mastering to a loudness target
   should take one setting, not a tuned pair of compressors. Landed as
   [`fx.limiter/1`](production.md#6-limiter--fxlimiter1) on 2026-10-04.
3. **Authoring density (F1–F3).** A hand-written track is largely note
   boilerplate. Candidates that keep the core declarative:
   - a chord leaf (several pitches in one note object);
   - a defined swing transform on `use` and `place`;
   - a drum-kit instrument that maps hit keys to synthesized drums.

   Each is a language change and needs its own proposal.
4. **Iteration speed (F7).** Profile the renderer, and let `analyze` render
   one region or score window.
5. **Level calibration (F15).** Publish each library instrument's loudness at
   `level = 1`, or calibrate `level` to a common reference. Done for
   std/studio, whose defaults share a −20 LUFS reference.
6. **Perception gaps (F13).** Measure the noise floor of quiet passages, and
   add the deferred score–audio checks (audible notes, sounding pitch).
7. **Delivery boilerplate (F11).** A built-in production schema reference
   instead of a copied, pinned file.

The [AI producer guide](ai-producer-guide.md) turns the working practice
from this trial into steps for the next AI producer.
