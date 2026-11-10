# Effects outside instruments: proposal

**Status:** accepted 2026-11-04, when the owner took every recommended
[decision](#decisions).
This is item 2 of
[what to build next](../reports/ai-production-trial-t8.md#what-to-build-next-in-order)
after the T8 trial. It comes from a review of production on 2026-11-03,
not from a listening complaint, and it closes F32 from
[T5](../reports/ai-production-trial-t5.md).

## Why

Seven trials finished their songs with the processing MaaC has outside
instruments: sums, gains, `core.pan/1`, `core.matrix/1`, `core.delay/1`,
`core.onepole/1`, and the production extension's EQ, compressor, reverb
and limiter. No listener asked for a missing effect; T5 recorded its
hand-computed echo as F32 and left it open as a convenience. The review
asked a different question: what can a producer not write at all?
By the [three questions](design-principles.md#evaluate-features-at-the-appropriate-boundary),
anything that changes the sound and cannot be written is the language's to
supply, since no tool can produce it.

The review checked each case against the 0.4.0 executable:

| A producer wants | What MaaC says today | Question |
| --- | --- | --- |
| Saturation on a drum bus or the mix | `synth.drive/1` exists, but only inside an instrument, whose shared graph hears only its own voices ([instruments](../instruments.md#instruments-controls-and-presets)) | Cannot write |
| A chorus, a flanger, a delay that wavers | `core.delay/1`'s length is fixed configuration, a whole number of frames | Cannot write |
| An echo in beats | `frames = 3/4q` is `E_UNIT`. "Afterlight" computes 17143 frames by hand for a dotted eighth at 126 bpm (F32). In a song whose tempo moves, as "Homeward"'s and the sanjo's do, a fixed length drifts off the beat | Cannot write |
| Moving a stereo part across the field: an auto-pan, a balance that changes | A stereo instrument into `core.pan/1` is `E_PORT_TYPE`; every built-in instrument is stereo. A fixed balance or width is one `core.matrix/1` whose coefficients the author computes. One that moves needs about seven nodes: two matrices to split the channels, two faders, two matrices to rejoin them and a sum | Can write, but the intent is lost |
| The same echo in another song | Processing outside instruments cannot be packaged or imported. "Afterlight"'s echo is five nodes and five connections of its own, which the next song writes again | Can write, but only by copying |

Instruments already solve the same problem for sound sources: a graph of
`synth.*` nodes behind named controls, declared once, imported with a pin,
and placed as one node. Effects need the same, with an audio input instead
of notes.

The goal: the processing above is written as an effect, declared once,
imported like an instrument, and placed as one node, with an echo that
counts in beats.

## The proposal at each boundary

| Part | Boundary | What changes |
| --- | --- | --- |
| 1. Effects | Language (sound-library extension) | An `effect` declaration: one graph that processes the audio at the node's `in`, behind named controls. A node places it with `effect = &id`, as it places an instrument |
| 2. Stereo nodes | Engine | `synth.balance/1` and `synth.width/1`, stereo in and out |
| 3. A delay in seconds or beats | Language and engine | `synth.delay/1`, whose length can be modulated and can be counted in quarter notes of score time |

## 1. Effects

An `effect` is a sound-library declaration, like an `instrument`, with one
graph and no voices:

```maac
effect echo {
  channels = 2;
  shared main {
    channels = 2; output = &out:out;
    node line { type = "synth.delay/1"; config = { channels = 2; clock = score; max = 2s; };
                params = { time = 3/4q; feedback = 0.35; damping = 0.5; }; }
    node wet { type = "synth.gain/1"; config = { channels = 2; }; params = { level = 0.25; }; }
    node out { type = "synth.mix/1"; config = { channels = 2; }; }
    connect in_line { from = &input:out; to = &line:in; }
    connect line_wet { from = &line:out; to = &wet:in; }
    connect dry { from = &input:out; to = &out:in; }
    connect wet_out { from = &wet:out; to = &out:in; }
  }
  control time { target = &main.line.params.time; default = 3/4q; }
  control return { target = &main.wet.params.level; default = 0.25; }
}

node lead_echo { effect = &echo; params = { return = 0.2; }; }
connect lead_in { from = &lead:out; to = &lead_echo:in; }
connect echo_mix { from = &lead_echo:out; to = &mix:in; }
```

- **The graph** follows the existing contract for an instrument's `shared`
  graph, unchanged: it runs once per sample from the project's reset origin
  through the tail, and `&input:out` is its input. In an effect, that input
  is the audio connected to the node's `in`.
- **Channels.** `channels` (1 or 2) is the output. An optional `input`
  (1 or 2, default `channels`) is the width of `in`, so a mono-in,
  stereo-out effect can pan.
- **Nodes.** Every node a shared graph allows: gain, the one-pole low-pass,
  the high-pass, `synth.svf/1` with ratio 0, `synth.drive/1`, mix, pan and
  the shared LFO; and the new nodes of parts 2 and 3. Voice-only nodes,
  such as oscillators, ADSRs and note sources, are rejected as they are in
  a shared graph. The graph stays a DAG.
- **Controls and presets** work as an instrument's: they keep their
  target's unit, range and rate, and can be automated, and modulated by a
  top-level `core.lfo/1` or `core.constant/1`. A score-clock LFO therefore
  moves an effect in time with the tempo, as it already moves an
  instrument's controls.
- **The node.** `effect` is a third alternative to `type` and
  `instrument`. Its ports are an audio `in` and an audio `out`. `in` sums
  its connections in connection-ID order, as `core.sum/1`'s does, and needs
  at least one, so an effect used as a send needs no sum in front of it.
  Like the production processors, it declares
  current-sample feedthrough from `in` and every parameter to `out`, and no
  technical latency, so it cannot break a feedback cycle in the project
  graph (§16).
- **Libraries.** A library declares and exports effects as it does
  instruments, and a composition imports them with a pin.
- **Resources.** An effect node counts as one of the project's 256 nodes.
  Its graph counts towards the instrument extension's aggregate graph
  limits, and its delay memory towards the production delay-cell limit.

## 2. Stereo nodes

Both take stereo in and give stereo out, sample their parameter each frame,
and are allowed in effect graphs and instruments' shared graphs.

- **`synth.balance/1`**, parameter `balance` (0; −1…1). The side away from
  the balance is lowered, and the other passes unchanged:
  `left × cos(π/2 × max(b, 0))`, `right × cos(π/2 × max(−b, 0))`. At 0 the
  signal passes unchanged; at 1 the left is silent.
- **`synth.width/1`**, parameter `width` (1; 0…4). With
  `M = (L + R)/2` and `S = (L − R)/2`, the output is `M + w × S` on the left
  and `M − w × S` on the right. At 0 it is mono, at 1 unchanged, and above 1
  wider.

With a score-clock `core.lfo/1` on an effect's balance control, a stereo
pad moves from side to side once a bar: one node and one modulation
instead of seven nodes.

## 3. A delay in seconds or beats

`synth.delay/1` is a delay line whose length is a parameter. Required
configuration: `channels` (1 or 2), `clock` (`seconds` or `score`), and
`max`, the longest delay in seconds, which sets its memory. Parameters:

| Parameter | Default | Range |
| --- | --- | --- |
| `time` | `0.5s`, or `1q` on the score clock | One frame up to `max` in effect |
| `feedback` | `0` | 0…0.99 |
| `damping` | `0` | 0…1 |

- **Seconds.** The line reads its input `time` seconds ago.
- **Beats.** On the score clock, `time = d` reads the input from the
  instant `d` quarter notes earlier in score time: at engine instant `t`,
  the instant `T(T⁻¹(t) − d)` of the tempo map (§5.1). An echo of a note at
  score position `q` therefore sounds at `q + d`, on the beat, even across a
  tempo ramp. During the tail the tempo map holds its last tempo, so the
  length holds the value it had at the score end. Reads before the reset
  origin are zero.
- **Modulation.** `time` is sampled each frame and may be automated and
  modulated, so an LFO on it makes a chorus or a flanger. The read falls
  between frames and is interpolated linearly, as `warp_rate` clips are.
  A length below one frame or beyond `max` is `E_RANGE` at the sample where
  it occurs, as the production processors' range errors are.
- **Feedback** stays inside the node, so the graph stays a DAG. Each frame
  the line reads its delayed value `r`, outputs it, and writes
  `input + feedback × f`, where `f = (1 − damping/2) × r + (damping/2) ×
  previous r`, the same damping that `fx.reverb/1` applies in its loop. The
  output is the delayed signal only; an effect mixes it with the dry signal.
- **State** resets at the reset origin and runs through the tail. Channels
  are independent.

## Effect on the existing songs

- **"Afterlight"** replaces its five-node echo, `echo_send`, `echo`,
  `echo_tone`, `echo_feedback` and `echo_return`, with one effect node whose
  time is `3/4q`; the arpeggio and the lead connect to it directly. Its
  length becomes exact instead of rounded to 17143 frames. Today one
  one-pole darkens both the loop and the return; in the effect the delay's
  damping darkens the loop and a one-pole the return, so the render is
  close to today's, not identical.
- **"Homeward"** and the sanjo could take an echo in beats through their
  ramps; neither uses one today.
- **Every other song** is unchanged and renders bit-identically.

A new example, `examples/effects.maac`, shows each part: a drive on a drum
bus, a chorus from a modulated delay, an echo in beats across a tempo ramp,
and an auto-pan of a stereo pad.

## Plan

1. **Effects (1)** with the nodes shared graphs already allow: the
   declaration, the node, controls, presets, library exports, the plan and
   the renderer.
2. **Stereo nodes (2).**
3. **The delay (3)**, first in seconds, then on the score clock.
4. **"Afterlight"'s echo** as an effect, and the new example.
5. **Documentation:** the instruments guide, the producer guide, the
   capability matrix, the specification's §1.3 and the CHANGELOG.
6. **T9,** a trial in a genre that leans on the mix, the owner's choice.
   This proposal is checked by what a producer can write; T9 checks whether
   the effects matter.

## Acceptance

- **Can write.** The example writes each row of the table in [Why](#why)
  as an effect node, without `core.matrix/1` and without computing frames.
- **On the beat.** On a tempo ramp, an impulse at score position `q`
  through an echo of `d` quarter notes comes out at `T(q + d)`, to within
  one frame.
- **Exact.** Each new node matches its formula on known inputs: impulses,
  sines, and a delay whose length moves. A node that a shared graph already
  allows behaves the same in an effect. Repeated renders are identical, and
  a render range matches the full render.
- **No loss.** Blind, "Afterlight" with its echo as an effect is heard as
  the same as, or better than, today's, with one pair sent twice with the
  letters swapped.
- **Unchanged songs.** Every existing example, fixture and trial song
  renders bit-identically.
- **Diagnostics.** Each new error has a test and an entry in the
  diagnostics table.

## Versioning

`effect`, a node's `effect` field and the three nodes make invalid source
valid and change the meaning of no valid source. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) they can
join `maac 1`. The plan gains effect instances and three processor
identities, which renderers that do not know them reject. `core.delay/1`,
`core.pan/1` and the production processors do not change.

## Rejected alternatives

- **`synth.*` nodes as project nodes,** the alternative in decision 1. It
  needs no new declaration, but an echo stays five nodes, cannot be
  packaged or imported, and every node counts towards the 256.
- **More fixed processors:** `fx.chorus`, `fx.saturator`, `fx.echo`. The
  gap is that nothing can be built, not a short list. Each fixed processor
  adds a specification, and the next request adds another.
- **An instrument with an audio input.** An effect has no notes, voices or
  capacity; joining the two would give one object two kinds of input.
- **Feedback through the graph,** the alternative in decision 2. Allowing
  cycles through a delay node would let a loop hold any filter, but it would
  change the DAG rule that every graph shares today.
- **Executable plug-ins.** Outside MaaC by the
  [design principles](design-principles.md#separate-language-completion-from-host-features).

## Not proposed

- **An envelope follower, a gate or an expander.** Synthesized parts have
  no bleed to gate. Ducking already works with a keyed `fx.compressor/1`,
  and a de-esser is a keyed compressor with an EQ on its sidechain path.
  These wait for a trial with recorded audio.
- **An LFO in beats inside an effect.** A top-level score-clock
  `core.lfo/1` on the effect's control does this.
- **Ping-pong and cross-channel feedback,** convolution reverb, pitch
  shifting and tape noise.
- **A standard effects library.** It should follow T9, as `std/studio`
  followed the trials that asked for its instruments.
- **A higher node limit.** An effect chain is one node; the limit is
  revisited if a trial reaches it.

## Cost and risk

- **No listener asked for this.** The gap comes from a review. Seven songs
  finished without these effects, and if T9 makes little use of them, the
  work came early. The defence is the second question: what cannot be
  written cannot come from a tool.
- **Two delays and two pans.** `core.delay/1` and `core.pan/1` remain the
  Core Audio processors for exact alignment and mono panning; `synth.delay/1`
  and the stereo nodes are for sound. The guides must say which to use.
- **Linear interpolation** dulls a modulated delay slightly and adds a little
  noise. T9 judges it; a better interpolator would be a new version.
- **Beat time costs work.** The score-clock delay evaluates the tempo map's
  inverse each frame. Work charges for the new nodes come from a benchmark,
  as the filter and drive charges did, and the delay memory counts against
  the existing production delay limit.
- **The shared-graph rules reach the project graph.** They were written for
  instruments, and effects reuse them unchanged. A rule that makes sense
  only for voices would surface here.

## Decisions

The owner accepted every recommendation on 2026-11-04 ("go with the
recommendations").

1. **Where effects live:** an `effect` declaration in the sound-library
   extension, placed as one node (recommended). The alternative allows the
   shared-graph `synth.*` nodes directly as project nodes.
2. **Feedback in the delay:** inside the node, with `feedback` and
   `damping` (recommended). The alternative allows cycles through the delay
   in effect graphs. The [effect loops proposal](effect-loops-proposal.md)
   reversed this on 2026-11-10, after T9 showed that a tape echo's repeats
   must pass through a drive and filters again.
3. **A delay in beats:** measured in score time, so an echo lands on the
   beat across a tempo ramp (recommended). The alternative counts beats at
   the current tempo, which drifts off the beat during a ramp.
4. **Stereo:** `synth.balance/1` and `synth.width/1` for effect graphs
   (recommended). The alternative gives `core.pan/1` a stereo mode, which
   changes a Core Audio processor and its conformance vectors.
5. **Listening:** one blind round on "Afterlight"'s echo, one pair sent
   twice with the letters swapped (recommended); then T9.

## Results

### Part 1: effects (2026-11-04)

`effect` declarations, effect presets and `node { effect = &id; }` landed
with the nodes a shared graph already allows. A node in an effect behaves as
in a shared graph, so the [instruments guide](../instruments.md#effects)
specifies effects by reference to that contract rather than restating it.

- **Same samples.** An effect of a `synth.gain/1` into a `synth.onepole/1`
  renders sample for sample as `core.gain/1` into `core.onepole/1`. With two
  connections into its `in`, an automated control and a score-clock
  `core.lfo/1` modulating it, it renders as a `core.sum/1` into the same
  chain with the same automation and modulation. A saved plan replays
  identically.
- **Libraries.** A library exports an effect and a preset; a song imports
  them with a pin and renders as with the effect declared locally.
- **Editing.** `maac patch` edits an effect node's controls and its graph;
  normalized, the node holds its controls' defaults, so an expectation can
  name them, as for an instrument node.
- **Production.** `maac deliver` and `maac analyze` accept projects with
  effects. The analyzer's roles are structural, so an effect fed by two or
  more connections is a mix point, as a sum is.
- **Unchanged.** Plans without effects carry no `effects` resource, and
  their bytes do not change.
- **Refused.** Native archives and the generic renderer refuse effect nodes
  with `E_CAPABILITY`, as they refuse kits.

### Parts 2 and 3: stereo nodes and the delay line (2026-11-04)

`synth.balance/1`, `synth.width/1` and `synth.delay/1` landed as the
proposal describes, with three decisions the proposal left open:

- **The delay is for effects only.** In an instrument it is `E_CAPABILITY`.
  A delay line in a voice graph would multiply its memory by the voice
  count, and only an effect's runtime holds the tempo map that a length in
  quarter notes needs. Balance and width work in effects and in instruments'
  shared graphs.
- **A full balance is exact.** `cos(π/2)` is about 6×10⁻¹⁷ in binary64, so a
  balance of ±1 silences the far side by definition rather than by the
  formula.
- **A quarter note is a unit of graph parameters.** A score-clock length,
  its control, and automation curves on that control take `q`.

Evidence:

- **Same samples.** A delay of 250 ms, a whole 12000 frames, renders sample
  for sample as `core.delay/1` with 12000 frames. Half a beat at 120 bpm on
  the score clock differs from it by at most 6×10⁻¹¹, the binary64 tempo
  arithmetic leaving a vanishing interpolation weight.
- **On the beat.** On a ramp from 60 to 180 bpm over 8q, a note at 2q echoed
  by one beat sounds within 3 frames of `T(3q)`, where an echo of one beat at
  the starting tempo would have come half a second early.
- **Balance and width.** A balance of 0 passes a stereo source unchanged,
  of 1 silences the left exactly and leaves the right unchanged, and of −1
  the reverse. A width of 1 leaves the source unchanged, of 0 makes it mono,
  and of 2 doubles the difference between its channels.
- **Movement and feedback.** An LFO on the length changes the output; a
  feedback of 0.9 keeps the line sounding through the tail after its input
  has stopped, where no feedback leaves it silent.
- **Identity.** `clock = seconds` and an omitted clock give the same
  execution identity in a delivery.
- **Work.** On an Apple M1, 40 chained lines in an effect took 14 ns per
  line and frame in seconds and 34 ns on the score clock, where
  `synth.drive/1`, charged 2 units, took 67 ns. They are charged 1 and 2.
- **Refusals.** A missing or out-of-range `max`, an unknown clock or config
  field, a length in the wrong unit, a feedback of 1 and a delay in an
  instrument are rejected at check time; a length beyond `max` stops the
  render with `E_RANGE`.
