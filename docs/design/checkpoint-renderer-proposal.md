# Checkpoint renderer: exact windows after an edit

**Status:** proposal, 2026-11-20. No engine code is written. Every size and
latency below is marked *measured* or *unmeasured*; the first step is a spike
that measures the unmeasured ones.

## Why

A production GUI re-renders what the user just edited. Today:

| Way to hear a two-second window of "Afterlight" | Time (measured) | Exact? |
| --- | ---: | --- |
| `render --start-frame/--end-frame` (exact excerpt) | 41 s | yes, but runs the whole plan |
| `render ... --preroll-frames` 1 s / 2 s / 4 s | 0.7 / 0.9 / 2.3 s | no: −16 / −73 / −110 dB (RMS, one window; another window gave −29 / −67 dB) |
| whole song, jit build | 8 s to 57 s for 78 s to 164 s of audio (2.3× to 9.3× real time) | yes |

The preview is fast and approximate. This proposal buys **exactness**, not
speed: after an edit, resume from saved engine state taken before the edit, so
the window's samples equal the full render's slice. It cannot be faster than a
preview with the same distance to render; it removes the preroll guess and the
approximation.

"Exact" here means *samples equal the full render's slice*. It does **not**
re-validate the whole plan the way the exact-excerpt contract in
[render-range.md](../render-range.md) does.

## The one question

After an edit, **from which frame is the old state untrustworthy?** Cloning
state is the easy half; on its own it does not help, because after an edit
every stored checkpoint belongs to the old plan. The design is the answer to
that question, an allowlist of edits that are safe to resume across, and a
test that fails when the answer is wrong.

## Design

### 1. Snapshot: what is mutable state

`DspEngine::reset` and `NodeState::reset` (src/dsp.rs) already list what a
render mutates; the snapshot is exactly that list, owned by the checkpoint
(`DspEngine<'a>` borrows its plan, so a checkpoint cannot borrow it):

- per node: `control_value`, `output`, `onepole_previous`, sine `voices`,
  `kit` voices, `kit_instrument`, `effect`, `eq`, `compressor`, `reverb`,
  `limiter`, `delay` (history, cursor, pending);
- per instrument: every voice's graph state (oscillator phases, plucked
  strings, modal banks, bows, envelopes, glides, expression runtimes), the
  shared graph, the control cache and the frames rendered ahead;
- engine: the output frame buffer.

Not state: noise nodes are a hash of the frame, audio clips and LFO controls
are functions of the frame, automation and tempo are read from the plan.

Types without `Clone` that must gain it or a snapshot method: `Limiter`,
`KitRuntime`, `EffectRuntime`, `InstrumentRuntime`, `KitInstrumentRuntime`,
`DelayRuntime`. (`GraphState`, `ProcessorState`, `Voice`, `Eq`, `Compressor`,
`Reverb`, `NoiseRuntime` already derive it.) *To verify in the spike:* that no
state hides outside this list, by snapshotting at a frame and checking that
the continuation is bit-identical.

### 2. Where checkpoints can be taken

Instruments render ahead, up to `BLOCK_FRAMES` = 128 frames, under the events
that existed when the block was rendered. A checkpoint taken mid-block would
carry frames computed under the old plan. Rule: **cap every block horizon at
the next checkpoint frame** (kit pieces included; to verify they obey the same
clamp). Rendering ahead is already bit-identical to frame by frame, so the cap
changes speed slightly and output not at all. At a checkpoint frame every
instrument buffer is empty.

### 3. Invalidation: an allowlist

Compare the old and new plan. Only these differences allow resuming; the
first affected frame is `affected_from`, and the usable checkpoint is the last
one at or before it:

| Difference | `affected_from` | To verify in code |
| --- | --- | --- |
| a note added, removed or changed | its note-on frame, or its line's first note-on if the instrument reads the line (`plays_lines`, handed over in `start_event`) | the expression `gate` is computed from the note's duration at note-on; joins; kit gates and chokes; `velocity_glide`, `pitch_glide` |
| a hit added, removed or changed | its frame, and any choke group it belongs to | kit chokes |
| an automation lane changed | the frame of the knot **before** the first differing knot (the segment before it interpolates toward the changed knot) | `AutomationLanes` ordering and the tail at `score_end_frame` |

Anything else is **frame 0, a full render**: graph, node parameters, patches,
modulations, controls, reset-rate values, `rate`, channels. Events are compared
by their frames, so a tempo-map edit that moves events invalidates from the
first moved event; an edit that changes the map but moves nothing is treated
as a full render until it is shown safe. The allowlist is deliberately small;
widening it is a separate decision per item, each with its own fuzz.

### 4. Restoring into a new engine

Build `DspEngine::new` for the new plan, then copy the checkpoint's mutable
state in. Before the first frame:

- `parameters_settled` and `parameters_valid_until` (computed from the old
  lanes) are reset, so the first frame re-evaluates;
- an instrument's voices may attach only if the new engine's
  `CompiledInstrument` is the identical program (compare by program identity),
  otherwise the node's checkpoint is rejected and the whole thing falls back
  to frame 0;
- frames rendered ahead are empty by §2.

### 5. API sketch (library; not the CLI)

```text
RenderSession::new(plan, spacing_frames)
session.update(new_plan)          // computes affected_from, drops later checkpoints
session.render(start, end, out)   // resumes from the last checkpoint <= start,
                                  // takes checkpoints as it goes, stops at end
```

Checkpoints live in memory, so this needs a long-lived process. The CLI cannot
keep them; saving them to disk is possible but not proposed.

## Cost (all unmeasured)

- **Bytes per checkpoint and clone time** depend on reverb and delay buffers,
  plucked-string rings, modal banks and live voices. Unknown. The spike measures
  them on Afterlight and Homeward.
- **Latency after an edit** is about (spacing + window) / real-time factor.
  With Homeward at 2.3× and 4 s spacing, a 2 s window is about 2.6 s; with
  Lantern at 5.1× about 1.2 s. Compare the preview's 0.9 s: checkpoints are
  not faster than a preview, they are exact.
- **Memory** is checkpoints × bytes per checkpoint. A 164 s song at 4 s spacing
  has 41. Spacing, eviction and keeping only checkpoints before the playhead
  are decided from the measured bytes.

## Tests (designed first)

1. **Resume equals uninterrupted.** For every example song, both builds:
   snapshot at frames F, restore into a fresh engine of the same plan, render
   to the end; every f64 bit equals the uninterrupted render.
2. **Edit fuzz.** Seeded random edits (move, add, remove, change a note;
   change an automation point), then a checkpoint-restore render must equal a
   fresh render of the new plan, bit for bit, on the interpreter and the jit.
3. **It fails when the bound is wrong.** A test that sets `affected_from` too
   late (for example the note-off instead of the note-on) must fail, so the
   fuzz is shown to detect the bug class that matters.
4. **Restore refuses.** A changed patch, a changed instrument program and a
   changed graph fall back to frame 0 and are reported as such.

## Phases

- **A. Spike, same plan only.** Add the missing `Clone`/snapshot, the horizon
  cap, `snapshot()` and `restore()`; measure bytes and clone time; tests 1.
  Exit: resume-equals-uninterrupted on every example, both builds, with the
  measurements written down. If bytes per checkpoint make 4 s spacing
  unaffordable, stop and revisit.
- **B. Edits.** Plan comparison, `affected_from`, the allowlist, tests 2 to 4.
- **C. Session.** Spacing and eviction policy, a background pre-render,
  streaming blocks for playback, GUI hooks, latency measured.

## Alternatives considered

- **Preview only (done, PR #66).** Fast and good enough to audition when the
  preroll is a few seconds; not exact, and the right preroll depends on the
  reverb tails and held notes.
- **Per-node output cache with dirty tracking.** An edit to one track
  re-renders that node and what is downstream; cached outputs of clean nodes
  are reused. Strong for editing one instrument, but the memory is large
  (about 63 MB per node for 164 s of float32 stereo, measured by arithmetic)
  and effects still need their state. Complementary to checkpoints, not a
  replacement; revisit after Phase B.
- **Checkpoints on disk.** Survives a restart, costs write time and space;
  only worth it if memory is the limit.

## Open questions for the owner

1. Does the GUI link this crate **in-process**? Checkpoints in memory need a
   long-lived process; the GUI is described as a separate application that
   reads and writes MaaC data.
2. What is the **memory budget** for checkpoints?
3. Which edits must be **fast**? Notes and automation are in the allowlist;
   patch, tempo-map and graph edits would be full renders.
4. Is **0.9 s approximate** (preview) acceptable for auditioning with an exact
   render following, or must the first sound be exact?

## Risks

- State hiding outside the inventory in §1 (mitigated by test 1).
- A note-on that reads the future in a way not on the §3 list (mitigated by
  test 2 and 3; every row of the table is verified in code before Phase B).
- Memory per checkpoint larger than expected (decided by the spike).
