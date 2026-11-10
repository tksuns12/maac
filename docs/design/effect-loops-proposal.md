# Feedback inside effects: proposal

**Status:** accepted 2026-11-10, when the owner took every recommended
[decision](#decisions), and landed the same day (see
[Implementation notes](#implementation-notes)).
This is item 2 of
[what to build next](../reports/ai-production-trial-t9.md#what-to-build-next-in-order)
after the T9 trial, and it closes F70. It reopens decision 2 of the
[effects proposal](effects-proposal.md#decisions) with what T9 found.

## Why

A dub's tape echo is a loop. Each repeat goes back through the tape, so it
comes out dirtier, thinner and darker than the one before. When the loop's
gain passes one, the echo grows until the tape saturates, and the engineer
pulls it back.

The effects proposal kept every graph free of cycles: `synth.delay/1` holds
its feedback inside the node, with `feedback` (at most 0.99) and `damping`.
T9 found what that costs:

| A producer wants | What MaaC says today | Question |
| --- | --- | --- |
| Repeats that wear, each pass dirtier, thinner and darker | Only `damping` acts inside the delay's loop. A drive or a high-pass after the delay acts once on every repeat, so all repeats wear the same | Cannot write in an effect |
| An echo that runs away | `feedback` stops at 0.99 and is linear. The swell rings long but never grows | Cannot write in an effect |
| The loop in the project graph | A cycle through an effect node is `E_ALGEBRAIC_LOOP`, and one frame of `core.delay/1` breaks it. "Low Tide" does this with 3 effects, 6 nodes and 6 connections, and each pass is one frame longer than the echo's time | Can write, but only by copying, in every song |
| One tape echo in a library | The loop lives in the project graph, so no library can export it | Cannot write |

By the
[three questions](design-principles.md#evaluate-features-at-the-appropriate-boundary),
the wear and the runaway change the sound, and an effect cannot write them.
The project graph can, but its loop has no name, cannot be imported or
reused, and has an extra frame on every pass that nobody asked for.

An effect refuses a cycle because each node's output in a frame depends on
its input in the same frame. A delay line's output does not. Its length is
at least one frame, so what it outputs in a frame was written in earlier
frames. For the same reason
[§16](../../MaaC-1-Specification.md#16-causality-and-graph-execution) lets
`core.delay/1` break a cycle in the project graph.

The goal: a tape echo that wears and runs away is one effect, and a library
can export it.

## The proposal at each boundary

| Part | Boundary | What changes |
| --- | --- | --- |
| 1. Loops through a delay line | Language and plan | A cycle in a graph is allowed when it passes through a delay line's audio input |
| 2. Mirrored width | Language | `synth.width/1` accepts a negative width, which mirrors the sides, so each repeat in a loop can cross the field |

## 1. Loops through a delay line

A graph's same-sample dependencies must still form no cycle. What changes
is that a `synth.delay/1`'s output no longer depends on its audio input in
the same frame. A cycle that passes through a delay line's `in` is
therefore allowed. The line's `time` still depends on whatever modulates
it.

```maac
effect tape_echo {
  channels = 2;
  shared main {
    channels = 2; output = &dark:out;
    node feed { type = "synth.mix/1"; config = { channels = 2; }; }
    node line { type = "synth.delay/1"; config = { channels = 2; clock = score; max = 4s; }; params = { time = 3/4q; }; }
    node heat { type = "synth.drive/1"; config = { channels = 2; }; params = { drive = 2; reference = 1/4; }; }
    node thin { type = "synth.highpass/1"; config = { channels = 2; }; params = { cutoff = 300Hz; }; }
    node dark { type = "synth.onepole/1"; config = { channels = 2; }; params = { cutoff = 3200Hz; }; }
    node regen { type = "synth.gain/1"; config = { channels = 2; }; params = { level = 0.55; }; }
    connect in_feed { from = &input:out; to = &feed:in; }
    connect feed_line { from = &feed:out; to = &line:in; }
    connect line_heat { from = &line:out; to = &heat:in; }
    connect heat_thin { from = &heat:out; to = &thin:in; }
    connect thin_dark { from = &thin:out; to = &dark:in; }
    connect dark_regen { from = &dark:out; to = &regen:in; }
    connect regen_feed { from = &regen:out; to = &feed:in; }
  }
  control time { target = &main.line.params.time; default = 3/4q; }
  control regen { target = &main.regen.params.level; default = 0.55; }
}
```

- **Each pass wears.** Every repeat goes through the drive and the filters
  again, so it comes out dirtier, thinner and darker than the one before.
- **It can run away.** With `regen` above 1, the repeats grow until the
  drive holds them. Below 1 again, they die away. Nothing limits a loop
  silently: one that grows without bound stops the render at its first
  nonfinite sample with `E_NONFINITE`, as today.
- **The echo's time is exact.** Every pass takes the line's `time`, with
  no extra frame.
- **Still refused.** A cycle that passes through no delay line, or one
  that closes through a line's `time`, is `E_ALGEBRAIC_LOOP`, as today. The
  message names the nodes of one such cycle in order. Today it says only
  that the graph must be acyclic.
- **Order.** In each frame, a delay line produces its output once whatever
  modulates it has run. It writes its input after the rest of the graph has
  run. In a graph without a cycle, every node receives the same values as
  today, so the graph renders sample for sample as today.

Only an effect may hold a delay line, so instrument graphs stay free of
cycles. The project graph's rule in §16 does not change: an effect node
still declares that its output depends on its input in the same frame.

## 2. Mirrored width

`synth.width/1` scales the side signal by `width`, which today is 0…4. The
range becomes −4…4, and a negative width mirrors the field:

- −1 swaps left and right.
- −1/2 sends three quarters of each side across. This is the 1/4–3/4 cross
  that "Low Tide" builds with `core.matrix/1` in the project graph.

Inside an effect's loop, each repeat then lands on the other side from the
one before.

```maac
node cross { type = "synth.width/1"; params = { width = -1/2; }; }
```

## Effect on "Low Tide"

Version 5 would rewrite only the tape echo:

- The effects `tape`, `wear` and `spread` and the project graph's `cross`,
  `gap` and `regen` become one effect, `tape_echo`. It has controls for the
  echo's time, the regeneration, the balance and the return. The desk
  places it as one node, with no loop in the project graph.
- Some automations and one modulation now target the new node's controls:
  - the echo's change of speed;
  - the three runaways;
  - the fade;
  - the LFO that swings the repeats.
- Every pass takes exactly the echo's 3/4q. In version 4, each pass is one
  frame (21 µs) longer.
  - Each pass, a repeat lands one more frame earlier than in version 4: by
    0.2 ms after ten passes. A runaway takes about eleven.
  - The ringing pitch of a runaway changes by less than a tenth of a cent,
    at the echo's 3/4q and at its faster 1/2q.
  - Version 5 therefore does not render sample for sample as version 4.

## Plan

1. **Loops (part 1):**
   - the graph rule, in compilation and in plan validation;
   - the cycle diagnostic;
   - the engine's order for delay lines.
2. **Mirrored width (part 2).**
3. **"Low Tide" version 5** in `examples/dub/dub.maac`.
4. **Documentation:**
   - the instrument contract's sections on effects, graph processors, the
     delay line and width;
   - a note at decision 2 of the effects proposal;
   - the producer guide, the capability matrix, the diagnostics table, the
     T9 report and the CHANGELOG.
5. **Next:** architecture candidate 2, which declares each synth processor
   once.

## Acceptance

- **Same samples.** Compare a loop in an effect with the same loop written
  in the project graph today, with the loop gain fixed and the effect's line
  one frame longer. The effect renders sample for sample as the project
  graph, one frame later, because the effect's line holds the frame that
  `core.delay/1` adds on each pass today.
- **Unchanged.** Every effect without a cycle, and every existing example,
  fixture and trial song, renders bit-identically. Their plan bytes and
  execution hashes do not change.
- **Wears and runs away.** T9's three probes, each rewritten as one effect,
  measure as they did in the project graph:
  - `tape-loop`: the centroid falls from 4026 to 1538 Hz pass by pass, and
    the share below 300 Hz from 22.7% to 0.4%;
  - `runaway-loop`: a regeneration of 1.25 grows and settles, held by the
    drive;
  - `pingpong`: the repeats alternate sides.
- **Refused.** Each of these is `E_ALGEBRAIC_LOOP` naming the cycle's
  nodes:
  - a cycle with no delay line;
  - a cycle that closes through a delay line's `time`.

  A width beyond ±4 is `E_RANGE`.
- **Diagnostics.** Each error has a test and an entry in the diagnostics
  table.

## Versioning

Both parts make invalid source valid and change the meaning of no valid
source. Today a cycle through a delay line is `E_ALGEBRAIC_LOOP` and a
negative width is `E_RANGE`. Under
[§26](../../MaaC-1-Specification.md#26-versioning-and-extensions) both can
join `maac 1`. A plan with a loop or a negative width is new. Renderers from
before this change reject it with `E_ALGEBRAIC_LOOP` or `E_RANGE`, which is
the right failure.

## Rejected alternatives

- **A delay node that does more:** a drive and filters in its own
  feedback, or `feedback` above 0.99. This is the fixed-processor path that
  the effects proposal rejected. A tape echo would get only what we thought
  of, and the next echo would need more.
- **A graph inside the delay node's feedback path.** It is the same loop,
  written a second way.
- **A separate kind of connection for feedback.** It would mark the loop in
  the source, but the delay line already marks it, and it would be one more
  thing to learn.
- **An effect node that breaks a cycle in the project graph.** The loop
  would still be written in each song, outside any library.
- **A one-frame delay node for effects,** like `core.delay/1`. Every pass
  would again be one frame long, and the loop would need two delay nodes
  instead of one.

## Not proposed

- **Loops in instruments.** Only an effect may hold a delay line, so an
  instrument's graphs stay free of cycles. A feedback voice, such as a
  feedback FM operator, would need a delay line in a voice.
  `synth.pluck/1` already covers the string.
- **A standard effects library.** The effects proposal said it should
  follow T9. "Low Tide"'s `tape_echo` becomes the first worked example.
  Putting it in `std` is sound design, which stays out of the trials
  (item 4 of T9's list).
- **Tape wow, flutter and noise.** An LFO on `time` already makes the line
  waver.
- **F69, a kit piece's own output.** It is item 3 of T9's list.

## Cost and risk

- **One trial asked for this.** Dub is where loops matter most, but any
  echo that should darken gains from it, and so do a feedback flanger and a
  comb filter.
- **It reverses a decision taken six days ago.** Decision 2 of the effects
  proposal was taken before any song used an echo. T9 is the first song to
  test it.
- **Loops can be unstable.** A loop with gain above one grows until
  something holds it, or until `E_NONFINITE`. That is the author's sound, as
  feedback in the project graph is today, and nothing limits it silently.
- **The engine's order changes for every effect.** A delay line writes
  after the rest of the graph. For a graph without a cycle the samples
  cannot change. The full test suite and the bit-identical examples check
  this.

## Decisions

The owner accepted every recommendation on 2026-11-10 ("응").

1. **Loops through a delay line** in effect graphs (recommended). This
   reverses decision 2 of the effects proposal. The alternative keeps every
   graph free of cycles and leaves the loop in the project graph.
2. **Mirrored width:** a negative `synth.width/1` (recommended). The
   alternative adds a stereo matrix node to the shared graph.
3. **Listening:** none, since version 5 moves each repeat by one frame per
   pass (recommended). Then architecture candidate 2.

## Implementation notes

Landed 2026-11-10.

- **The rule.** The graph's order leaves out an audio connection into a
  `synth.delay/1`, and the engine writes each line after the rest of the
  graph has run. A refused cycle's message names it, for example
  ``same-sample cycle feed -> regen -> feed: a cycle must pass through a
  delay line's `in` ``.
- **Same samples.** The test of the acceptance holds exactly. With a loop
  gain of 0.9 and a line of 376 frames in the effect against 375 in the
  project graph, the effect's output is the project graph's one frame
  later, sample for sample.
- **Unchanged.** All 71 checked-in compositions that render, including
  "Low Tide" version 4 and `examples/effects.maac`, render byte for byte as
  they did before this change. `showcase` exceeds the plan size limit
  before and after, and `examples/sounds/studio.maac` is a library.
- **T9's probes.** `tape-loop`, `runaway-loop` and `pingpong`, each
  rewritten as one effect, measure as the project graph's loops pass by
  pass. Measured over eight passes of 3/4q:
  - `tape-loop`: the centroid falls from 1679 to 1247 Hz, and the share
    below 300 Hz from 42.7% to 0.8%;
  - `runaway-loop`: the peak falls from 0.37 to about 0.2 and holds there,
    where a loop gain of 0.7 falls to 0.005;
  - `pingpong`: each pass is entirely on the side opposite the last.

  T9's own figures, 4026 to 1538 Hz and 22.7% to 0.4%, were measured over
  other windows.
- **"Low Tide" version 5.** The tape echo is one effect, `tape_echo`, and
  one node. The desk has 6 fewer nodes, 7 fewer connections and 2 fewer
  effects. Compared with version 4:
  - the waveforms differ, because each repeat drifts by one frame per
    pass and its high partials soon fall out of phase;
  - every section's loudness and every octave band of the master are
    within 0.01 dB of version 4's. The whole song measures −17.42 LUFS and
    −1.10 dBTP, as before, and the release delivery passes;
  - the echo's return alone is within 0.11 LU per section, and within
    0.06 dB per octave band.
