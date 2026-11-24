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

## Phase A results (measured 2026-11-20, release build, Apple silicon)

What was built: `DspEngine` and every runtime type under it derive `Clone`;
audio clip transports sit behind an `Arc` (they are stateless, so a snapshot
shares them); block horizons stop at checkpoint multiples; a checkpoint refuses
to be taken if any instrument still holds frames rendered ahead;
`render_with_checkpoints(every, cb)`, `DspEngine::restore(&checkpoint)`, then
`render_span` from the checkpoint frame. This first snapshot is a **whole-engine
clone**: it also copies the immutable tables (events, connections, node
parameters), so the numbers below are an upper bound for the "mutable state
only" snapshot in section 1.

Correctness: on every example in `examples/` that renders on its own (the
asset-backed ones and the subdirectories included, loaded through the real
bundle loader; only `showcase`, whose plan exceeds the song byte limit, and the
`sounds` libraries are left out) and on the five songs, taking checkpoints does
not change the render, and resuming from a checkpoint reproduces every f64 bit
of the uninterrupted render. Spacings: a fifth of the song (odd, so off the
block size) for examples up to 20 s, 1009 and 4801 frames on `hello` and
`core-delay`, and 4 s plus 37 frames for the longer examples and the songs
(those run in the ignored test). A control test shows a cold engine at the same frame does not match.
Tests: `tests/checkpoint_resume.rs`.

Because the snapshot is a derived deep clone, these tests pass whatever
section 1 lists: they do **not** verify the inventory of mutable state. That
check belongs to Phase B, when state is copied into a new engine instead of
cloned.

Cost (`tests/checkpoint_cost.rs`, spacing 4 s):

| Song | checkpoints | per checkpoint, mean / max | all checkpoints | slowest clone |
| --- | --- | --- | --- | --- |
| Homeward | 31 | 3.4 / 5.0 MB | 106 MB | 0.73 ms |
| Afterlight | 40 | 3.9 / 4.0 MB | 157 MB | 0.87 ms |
| Lantern | 32 | 3.1 / 3.2 MB | 97 MB | 0.57 ms |
| Ferry | 34 | 1.7 / 1.8 MB | 57 MB | 0.43 ms |
| Lofi | 19 | 1.1 / 1.2 MB | 21 MB | 0.21 ms |

A freshly built engine is already 0.8 to 3.6 MB of that. Part of it is the
immutable tables and part is mutable buffers allocated at construction
(reverb and delay history), so the size of a mutable-only snapshot is not
measured; it is somewhere between a few hundred KB and the numbers above. At
these sizes the exit condition ("4 s spacing is unaffordable") is not met by a
wide margin even for the whole-engine clone. Taking a
checkpoint costs under a millisecond; the horizon cap changed no output bit.
A `Checkpoint<'a>` holds a `DspEngine<'a>` and so borrows the plan, which
section 1 said it would not; Phase B's `session.update(new_plan)` has to
copy mutable state into an engine of the new plan, not reuse the clone.

A restored engine lays down no checkpoints while it resumes, so the section 5
session ("takes checkpoints as it goes") needs that added in Phase B or C.

Misuse is refused rather than approximated: a restored engine takes no
checkpoints, serves one render from its checkpoint frame, refuses any other
span without losing its state, stays refused if that render fails part-way
(until `reset` or `restore`), and spacing under 32 frames is rejected.

Not measured: render time with versus without the cap (the render times in
that run include the cap and checkpoints but were not compared with a plain
run on a quiet machine), and the cost table on the jit build.

## Phase B results (measured 2026-11-20)

Built: a `Checkpoint` now owns only node state and borrows no plan;
`DspEngine::restore_from` copies it into an engine built for any plan whose
graph matches, and `CheckpointSet` (`src/dsp/checkpoint.rs`) decides which
checkpoint an edit leaves valid. Phase A's clone tests could not check the
inventory of mutable state; restoring into a freshly compiled second engine
does, and passes on every example and song (nothing outside the nodes is
mutated by a render).

What an edit may keep (anything else renders from frame 0 and names what
changed):

- A note or hit that differs (found by node and address, compared on what the
  engine schedules) bounds at its own on frame. For an instrument that reads
  connected lines (a bowed section) the bound is also the on frame of the head
  of the note's line, in both plans, because the head reads the whole line when
  it starts; for any other instrument a joined note's effect on the note
  before it happens at the joined note's own on frame.
- An automation lane that differs bounds at the knot before the first one that
  differs, or at the lane's start when its curve, anchor or base value differ,
  or a lane is added or removed. A change in the value held after the score
  ends counts only from the score's end.
- Not compared: source spans, source hashes and execution identities (they
  change with every edit of the text and a render never reads them), regions.

Evidence (`tests/checkpoint_edit.rs`; every resumed tail equals a fresh render
bit for bit, with a checkpoint exactly at the bound and on a 4801-frame grid):
generated songs with a bowed line (joins, slides, slurs), polyphonic notes, a
reverb, a delay loop and two automation lanes (some starting late), edited at
random in fifteen ways. The default run is 26 accepted edits; the ignored
sweep over 30 songs is 372 accepted edits, 312 resumed partway, 0 full renders
and bit-exact on the interpreter, and the default run also passes on the jit
build. A checkpoint placed one frame to 97 frames after the bound gave wrong
frames or an error for 320 of 360 note edits, so the fuzz can see a bound that
is too late. An independent review (outside the repo) resumed 41 edits on
plans with modulation, kits with choke groups, non-bowed connected voices,
expressions and several lanes on one parameter, from the bound and from
earlier checkpoints, on both builds: no unsound bound.

Two things the first version got wrong and the generated songs hid, now
covered by tests that fail without the fix: provenance in the production
settings and in instrument source spans made every edit to a real song a full
render.

Latency (release, interpreter, Afterlight, 164 s, 4 s checkpoints, edits to
note velocities around 43 to 46 s): compile 0.09 s, diff and restore 0.03 to
0.04 s each, rendering from the checkpoint to two seconds after the edit 1.4 to
2.1 s, so about 1.5 to 2.3 s to hear an edit, against 12 s to render the same
frames from frame 0. Measured on the jit build earlier: 1.1 to 2.3 s, with
0.13 s each for the diff and restore (building the jit engine). Only edits
near 43 to 46 s were measured.

Not covered: edits that combine several changes beyond what the fuzz does
(one at a time), asset-backed legacy kits, sample instruments, effect
programs, tempo ramps with score-clock lanes; the diff builds a second engine
(0.03 to 0.13 s) that `resume` then builds again, which Phase C can share.
Slack that remains: a note edit bounds at the note's on frame even when the
change is inaudible until later.

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

Answered 2026-11-20:

1. **In-process or separate?** Not decided; the owner has not chosen the GUI's
   language. Checkpoints live in memory, so they need either the GUI linking
   this crate (Rust) or a long-lived MaaC process the GUI talks to. Phase A
   does not depend on the answer.
2. **Memory budget.** Unknown to the owner. Decide after the spike measures
   bytes per checkpoint.
3. **Which edits must be fast?** The most common ones. Notes and automation
   are in the allowlist; patch, tempo-map and graph edits are full renders
   until usage shows they matter.
4. **Approximate first?** Yes. After an edit, play the approximate preview
   (PR #66) at once and let the exact render follow.

## Risks

- State hiding outside the inventory in §1 (mitigated by test 1).
- A note-on that reads the future in a way not on the §3 list (mitigated by
  test 2 and 3; every row of the table is verified in code before Phase B).
- Memory per checkpoint larger than expected (decided by the spike).

## Phase C decisions (2026-11-21)

- The GUI uses the library in its own process, so a session is a library type
  and checkpoints stay in memory.
- An edit is first heard through the approximate preview; the session only
  produces the exact render and the GUI swaps it in.
- The memory budget follows what music programs do: a size that scales with the
  machine and that the user can change. The library has no way to read the
  machine's memory, so it defaults to 1 GiB and the caller passes its share.

## Phase C1 results (synchronous session)

`RenderSession` (src/dsp/session.rs): `update(plan)` only records the plan;
`render(start, end, cb)` builds one engine, compares the plan with the one the
checkpoints belong to (so several edits before a render cost one comparison),
resumes from the last checkpoint at or before `min(bound, start)`, and keeps
the old checkpoints up to the bound plus fresh ones for the frames it redid.
A render that fails leaves the session unchanged.

- Checkpoint size is not measured: the state is spread over every node (on
  Afterlight the largest node is 23% of 1.17 MB of node state), so the session
  counts each checkpoint as 8 MiB, above the 5.0 MB maximum measured on the
  five songs. The budget is therefore a count, `budget / 8 MiB`.
- Thinning removes the checkpoint whose loss widens its gap least, weighted by
  distance from the last render's start, so the far parts of the song get a
  coarser grid. The last checkpoint's gap runs to the song's end: a first
  version measured it to a made-up point and erased the whole tail after the
  playhead (found by the thinning unit test).
- Memory during a render is up to twice the budget (old and fresh checkpoints
  together). A range at a tight spacing would hold far more fresh checkpoints
  before thinning could drop any, so the spacing widens until the range yields
  at most the budget's count.
- Mutation checks, each caught by the named test: keeping checkpoints past the
  bound (the chain test, which needed a short render followed by another with
  no edit between; the first version did not catch it), keeping only the first
  of several updates, trusting a `Full` reuse (tempo change), and ignoring the
  channel count when skipping frames (stereo example). Review of the first
  version found these test gaps, each since closed.
- Not done: a generation number (nothing runs concurrently yet; C2 adds it),
  background pre-render (C2), playback streaming and latency numbers (C3).

## Phase C2 results (background pre-render)

`BackgroundSession` (src/dsp/background.rs) wraps a `RenderSession` and one
worker thread. `update` puts the plan in a slot and returns; `render` runs on
the caller's thread and the worker stops while it does; the worker adopts the
latest plan (dropping the checkpoints the edit invalidates), then renders from
the last checkpoint to the end, publishing each checkpoint as it is taken
(`DspEngine::render_span_with_checkpoint_sink`).

Correctness rests on one rule: the session counts its `update` calls under the
state lock, and a checkpoint from the worker is accepted only if the count is
what it was when that stretch of work began. A separate generation counter
only tells the worker to stop early. The test holds the worker just before a
commit, gives and renders an edit that changes the sound before that frame,
then lets the worker go; with the count check removed the test fails
(mutation checked).

Measured on Afterlight (164 s, release, Apple silicon, spacing 4 s, velocity
edits piled up on the text, 2 s window starting at the first affected frame):

| | |
|---|---|
| worker fills all 40 checkpoints from nothing | 49.8 s |
| edit at 45.7 s, worker idle | first sound 0.60 s, window 2.2 s |
| edit at 43.8 s, worker idle | first sound 1.37 s, window 2.1 s |
| edit at 43.3 s, worker still busy | first sound 1.22 s, window 1.9 s |
| edit at 42.9 s, worker idle | first sound 1.04 s, window 1.7 s |
| edit at 0.0 s | first sound 0.03 s (nothing before it to render) |
| edit at 44.8 s straight after the edit at 0.0 s | first sound 10.1 s |

The last row is the limit of the design: an edit at the very start invalidates
every checkpoint, the worker must render from frame 0 to rebuild them, and an
edit that arrives before it has reached the edit's position renders from frame
0 itself. A first run read 82 s for the fill because other jobs were running.

Review of the first version found no way for a stale checkpoint to enter the
set, and these problems, since fixed:
- a set at its budget made the worker render the same tail at every wake and
  keep nothing (a checkpoint added at the tail pushed another out); the worker
  now leaves a full set alone unless the tail gap has grown past twice the
  average one;
- a panic in a render's callback leaked the foreground count and stopped the
  worker for good (now a guard); a panic in the worker was silent (now caught
  and reported in `status().last_error`);
- the generation was read after the plan was taken up, so an update landing in
  between did not stop the stretch (now read first);
- `status` waited for the render's lock (now read from copies kept in atomics);
- `render_span_with_checkpoints` left partial checkpoints in the engine after
  an error.
Tests added for each; mutation checks caught: no generation stop, adoption
that keeps every checkpoint (caught by the stop test, not by the test written
for it, which passes either way), no join on drop, no skip for a full set, no
thinning on insert, and no retry after a cancelled stretch.

Not done: the worker's OS priority is not lowered (it only yields to
foreground renders); an update during a stretch of work always restarts it from
the last committed checkpoint even if the edit is late; the plan is cloned
for each stretch.

## Phase C3 results (playback while editing)

`BackgroundSession::play(start, config)` returns a `Playback` (src/dsp/playback.rs).
One producer thread renders chunks of the latest plan into a ring that stays
`lead_frames` (default 65 536) ahead of the reader; `read` copies what is ready
and zero-fills the rest (an audio callback never renders). The ring is a
`Mutex<VecDeque>` held for microseconds, not a lock-free queue: simpler to
check, and the critical sections are a memcpy. The library does not open an
audio device; the GUI calls `read` from its device callback.

Engine support: `DspEngine::render_chunk` renders an exact render in pieces
(each chunk's block horizons stop at its end, so no instrument holds frames
ahead there) and `chunk_checkpoint` snapshots the state between chunks.
Cutting a render into chunks of 512 frames to the whole song costs nothing
measurable (Afterlight, first 20 s: 4.13–4.28 s whatever the size, 4.28 s as one
span).

What an edit does to a playing stream, decided by `CheckpointSet::reuse_with`
against the plan the producer is playing:
- the effect starts at or after the producer's position: the engine's own
  state is a checkpoint of the new plan too, so the new plan takes over at the
  next chunk (zero catch-up);
- the effect starts earlier: the new plan has to be rendered up to the
  playhead. The producer keeps filling the ring with the old plan (marked
  `stale`) while a second thread renders the new plan forward, from the
  latest of the session's checkpoints and snapshots the playback took of its
  own engine (every 24 000 frames, the last 8 kept). When it is within 4 chunks
  and the ring has two chunks to spare, the producer holds still, the
  catch-up finishes the last of the distance, and the sound switches;
- the plan is now shorter than the playhead: the playback ends.
`PlaybackStatus::switches` lists the frame each plan began at, so the GUI knows
when an edit became audible. `seek` drops the ring and restarts from the nearest
checkpoint, silent until it has caught up.

Every frame handed out is a frame of an exact render of one plan: the tests
compare every f64 of what a reader got against fresh renders of the plans the
playback says it played (reads of 1, 17, 4096 and 100 000 frames; chunks of
64 to 4801; edits from 15 generated edit kinds at the start, middle and with
the worker running; edits before the playhead; a tempo edit that shortens the
song, early and past its new end; seeks forward, back and past the end; an edit
then a seek). Mutation checks, each caught: taking a switch whenever the edit
is earlier than the playhead without catching up, keeping the switch list over
a seek, accepting a chunk rendered before a seek, ignoring the edit bound when
choosing a checkpoint, not stopping block horizons at chunk ends, letting the
old plan starve the catch-up, not watching for edits, and switching one frame
late. Also caught: not clearing the parameter caches between chunks (found when a
leftover mutation broke the seek tests).
A first version let the old plan starve the catch-up when the reader outran the
producer, and the test reader (twice real time) flaked once in three before
the producer was made to leave the catch-up the time above half a ring.

### Measured (release, Apple silicon, 8 cores; a reader pulling 512 frames at the speed the sound plays)

The machine was shared: the load average was 26–37 during the runs, so
the catch-up times are upper bounds. Playhead at a quarter of the song, the
worker finished (checkpoints every ~4 s), edit made 4 s after play started.

| | Afterlight (164 s) | Lantern (129 s) | Homeward (128 s) |
|---|---|---|---|
| first sound after `play` | 0.42–0.46 s | 0.11–0.13 s | 1.97–2.02 s |
| edit with effect far ahead: new sound after the playhead | 1.4 s | 1.4 s | 1.4 s |
| effect 0.5–3 s behind the playhead | 16–20 s | 2–3 s | 4–6 s |
| effect 5 s behind | 19 s | 7.5 s | 18 s |
| effect at the very start | 24 s | 15 s | 42 s |

In the second row, 1.4 s is the ring: that much audio was already rendered
with the old plan. In the other rows the figure is how long the old sound kept
playing (the producer's catch-up time) until the new plan reached the playhead. The catch-up runs at the song's own render
speed: Afterlight and Homeward render only about 1–2× real time in the
sections measured, so a thread chasing a playhead that moves at 1× gains
little (Lantern, faster, catches up in seconds). Before the second thread and
the playback's own snapshots the same Afterlight edits took 30–78 s.

### Not done / limits
- An edit that changes the sound behind the playhead is heard late on a heavy
  song (table above). Candidates: denser checkpoints near the playhead,
  splitting the catch-up across more threads, or the GUI showing the
  approximate preview of the new plan meanwhile (phase C's earlier decision).
- Edits within the last `lead_frames` of the song may not be heard in this
  playback (the producer has already put the end in the ring and stops
  watching for edits).
- The producer and catch-up threads and the pre-render worker can be three busy
  cores; `pause_prerender` holds the worker still while the playback lives.
  Thread priorities are not changed.
- The playback's own snapshots are 1–5 MB each (up to 8 per playback) and
  are not counted in the session's memory budget.
- The tests' wall-clock-sensitive part is the reader's pace (twice real
  time); a machine too slow to render at 2× real time would make them fail.
- Measured on three songs, one run each, on a loaded machine.

### Review of the first version (fixed)
An independent review found: an edit that changes the channel count garbled the
stream (now the producer reports an error and waits); a lead-in edit was
labelled with the old plan; a lead of one chunk never handed over; a longer plan
was cut at the old end; a producer error killed the playback for good (now a
seek or an update restarts it); a stale `finished` after a seek; and a switch
labelled with an older plan when two updates raced (the generation is now read
under the slot lock). Regression tests cover the first five; mutation checks
caught three of the six fixes. Not caught, because they are timing races: the
generation read under the slot lock, the `finished` write after a seek, and the
hold threshold. Still open: rapid edits restart the catch-up each time (a slider
drag may not switch until it stops); `read` freed chunk buffers on the audio
thread (fixed afterwards: finished buffers go back to the producer, and
`tests/playback_realtime.rs` counts frees and allocations on the reading
thread); a panic in the catch-up's producer side is now stopped by a guard but
untested.

## Phase C4 results (approximate sound while the exact render catches up)

An edit that changes frames behind the playhead used to leave the old plan
playing until the exact new plan reached the playhead (16-42 s on heavy songs).
`catch_up` now builds a second engine for the new plan and, after the edit,
renders it cold from `preview_preroll_frames` (default 24 000) before the playhead
up to the playhead, in the producer's spare time (before the old plan's refill,
unless the ring is nearly empty). From there the ring is filled with that
approximate sound (`PlaybackSwitch::approximate_from = Some(cold frame)`,
`PlaybackStatus::approximate`) until the exact render hands over (a switch with
`approximate_from = None`). The approximation is the engine's existing preview, so
a stretch can be checked bit for bit against one preview render from its cold
frame; `DspEngine::render_preview_chunk` renders it in pieces (the engine remembers
where the preview began, so a continuing chunk skips what a render from there
never started).

Tests: a preview cut into chunks of 1 000 to 70 000 frames equals one preview
render; misplaced preview chunks are refused; a playback with an early edit has
exactly the switches exact, approximate, exact, each stretch equal to its fresh
render. Mutation-checked (8 mutants of the preview path: 5 caught, 2 equivalent,
1 not testable without timing: the old plan refills the ring first when it runs
nearly empty).

Measured (Afterlight, release, loaded machine: the worker took 104 s to fill, 48 s
when quiet): an edit behind the playhead is heard approximately 0.3-0.7 s after
the edit (before: 16-25 s) and exactly after 23-33 s. The "underruns" first
reported for two runs (426 and 655 reads) were most likely the start-up wait for
the first sound, which the counter included; the harness now counts underruns
before and after the edit apart. Remeasured, load average 10-140 on 8 cores:

| song | first sound after play | approximate after the edit | exact after the edit | underruns after the edit |
|---|---|---|---|---|
| Afterlight | 0.43 s | 0.38 s | 18-23 s | 0 |
| Homeward | 2.0 s | 0.81-0.88 s | 20-24 s | 0 |

Approximate rendering alone runs 1.7-6.5x real time when the machine is quiet
(Homeward 1.7-2.3x, Afterlight 3.9-6.5x); skipping the reverb changes nothing, the
time goes to the instruments. Holding the exact catch-up back until the
approximate sound is up saved about 3 s of the exact time but is within the noise
of the machine's load, so it was not kept.

Limits: the approximation lacks reverb and delay tails from before its cold start
and restarts sounding notes; an edit that arrives while it plays starts again from
the newest plan at the end of the ring (since C5 through the same gate as a start,
so the sound goes on approximately from there when the checkpoint is far, and is
silent until the exact render has caught up when it is near: it may run dry); rapid
edits still restart the catch-up.

## Phase C5 results (approximate start)

A start or a seek renders silently from the nearest checkpoint up to the playhead
before the first frame can be played: up to the checkpoint spacing (4 s in the
measurements), 1.8 s on Homeward 3.5 s past a checkpoint. When the playhead is more
than three prerolls (`START_GATE_PREROLLS`, 72 000 frames by default) past its
checkpoint, `run_plan` now calls `start_approximately`: the catch-up of C4 with no
old plan. The exact engine restored from the checkpoint chases the playhead on its
own thread; a second engine renders the plan cold from `pos - preroll` and its sound
fills the ring; when the exact engine is within a few chunks, it takes over.
Differences from an edit's catch-up: the preview always advances (the ring is empty,
so it is always "starving"); the exact engine may take over before the approximate
sound is up (nothing was heard, no switch is recorded for the approximation); the
first entry of `switches` is relabelled approximate instead of a second entry added
(`record_switch` corrects an entry at the same frame while nothing of it is in the
ring). An edit during it starts again from the newest plan. `approximate_start:
false`, or `preview_preroll_frames` 0, gives the old contract: `play` and `seek`
are exact from the first frame.

Measured (release, 8 cores, load average 50-70 from other work; a reader pulling 512
frames at the speed the sound plays; first sound / until exact):

| song | past a checkpoint | exact start | approximate start |
|---|---|---|---|
| Homeward | 1.0 s | 0.69 s | (below the gate) |
| Homeward | 2.0 s | 1.15 s | 0.57 s / 3.1 s |
| Homeward | 3.5 s | 1.83 s | 0.56 s / 4.5 s |
| Afterlight | 2.0 s | 0.31 s | 0.14 s / 0.58 s |
| Afterlight | 3.5 s | 0.53 s | 0.14 s / 0.79 s |

The gate: at 1.0 s the approximate start would have saved about 0.1 s for a
degraded start; the gain is worth it from about 1.5 s. (The 2 s first sound quoted
in the C4 notes was measured at a load average of 100-140.)

Tests: far start, far seek and a far start from a real checkpoint are approximate
then exact, each stretch equal to a fresh render (the approximate stretch to a
preview from its cold frame) and each ending with an exact switch at a frame that
exists (a hand-over at the end of the song would also give the shape "approximate,
exact"); a near start is exact and single; both off switches give an exact start; an
edit that arrives while the approximate sound is playing is followed; a preroll of
`u64::MAX` does not overflow the gate. The far tests assume the approximate sound
is up before the exact render is within a few chunks: the margin is tens of
milliseconds against a render of 30 000 to 160 000 frames, which a machine stalling
the producer for that long would break.

Mutation-checked (12 mutants of the start path): 7 caught, 3 equivalent, 2 not
caught. The equivalent ones: preroll 0
without its guard (no preview is made, the exact render starts at once, the same
sound; the guard avoids a thread and a spin); not noting the plan on a superseded
start (the restart relabels it); starting the chase from frame 0 instead of the
checkpoint (the same sound, only later). The two not caught: dropping the reset of the
approximate mark in `note_plan` (an edit would have to arrive in the moment between
the preview going live and its first chunk reaching the ring); dropping the "nothing
heard yet" waiver of the spare-sound condition (needs the exact engine to arrive
before the approximate one, which no deterministic test can force).

A review also found that `BackgroundSession::update` released the slot lock before
counting the plan, against its own comment: a playback taking the slot in between
could name plan N+1 as plan N. The plan and the count now go in under one hold of
the lock. No test: the window is a few instructions wide.

Limits: after a start the sound is approximate for 0.6-4.5 s on these songs (the
time the exact render needs from the checkpoint); a start is not approximate below
the gate; the approximation lacks reverb and delay tails from before the cold frame.

## Phase D results (parallel render)

After an edit behind the playhead the exact sound came back 20-96 s later, because
one thread renders Homeward at 0.64-2x the speed of the sound and the catch-up
time is the gap divided by (speed - 1). The render is now spread over threads
without changing a sample.

How it works:

- **Instruments side by side.** Where several instruments that nothing modulates
  need a new block at the same frame, and the events at that frame have been
  delivered, the blocks render at once (each runtime is moved to a worker and
  back; no shared state). Block ends are clamped to multiples of 128 frames so
  the refills of different instruments coincide. The engine then reads the
  blocks in its own order, and an error is kept in the runtime until the frame
  the serial engine would have reported it at.
- **Thread limit.** An engine shares the instruments of a batch out in at most
  `threads` jobs, the costliest first (each to the job with the least so far,
  by the time its last block took), so one engine never has more than
  `threads` jobs in flight.
- **Pool.** Persistent threads (cores - 1, at most 7) with one shared queue. A
  caller runs queued jobs while one of its own results is missing and goes on
  with its work as soon as the last is in, so a job may use the pool itself and
  a caller is not held for other callers' jobs. A job that panics gives an
  error, not a hang, and its instrument goes back where it came from. A caller
  helps with its own pieces only, so it is never held by a long piece of
  another engine.
- **Only when it pays.** Handing a batch to other threads costs tens of
  microseconds. A batch is parallel only when the last known fill times of its
  instruments, less the costliest, add up to 100 us (`MAAC_PARALLEL_MIN_NANOS`;
  the tests set 0). Unknown times render serially, which measures them.
- **Default.** An engine renders on `min(cores, 4)` threads;
  `MAAC_RENDER_THREADS` or `set_render_threads` change it.

Measured (release build, Homeward; the final build, on a quiet machine, load 6-8):

| | 1 thread | 4 threads |
|---|---|---|
| full render | 69.6 s, 73.0 s | 34.0 s, 34.0 s |
| exact sound after an edit 5 s behind the playhead | 96.4 s (3529 underruns; load 7.6, an earlier build) | 7.43 s (0 underruns) |
| 31 checkpoints by the worker | 70.8 s | 34.0 s |

The 1-thread playback run was made at a load average of 7.6, the first 4-thread
playback run (an earlier build) at about 180 and gave 11.75 s. The full
render's output has the same SHA as before on 1 and 4 threads. Before the
review fixes the full render took 31.8 s: sharing a batch out in at most
`threads` jobs and not splitting voices inside it cost about 7%.

Repeat on the final build (load 6-8, so times wander): Homeward 1 thread 19.3 s
and 0 underruns (the 96.4 s above was a single run on an earlier build and is not
a baseline to quote), 4 threads 7.3 s; the full render's SHA is 98701218 on 1
and 4 threads at the default gate (69.2 s, 39.7 s). Afterlight with the gate,
exact sound after the edit and underruns after it: 4 threads 15.5, 20.3, 15.8 s
(0, 0, 0), 3 threads 17.4 s (0), 2 threads 18.1 s (0), 1 thread 17.7 s (0).
Parallel rendering gains little on Afterlight; the gate keeps it from costing.

Afterlight (quiet machine, load 3-5), exact sound after an edit 5 s behind
the playhead. Without the work gate parallel rendering made it worse: 1 thread
17.7 s and 0 underruns; 2, 3 and 4 threads 24-25 s with 363, 401 and 474
underruns (a priority queue alone changed nothing: 705). With the gate, 4
threads: 15.5 s, 0 underruns, worker 40.0 s (1 thread 47.8 s). Homeward with
the gate: 7.27 s, 0 underruns, worker 33.1 s.

Tests: `tests/parallel_render.rs` (whole renders, a cold preview in pieces and
checkpoint + resume on 1, 2, 3, 4 and 8 threads), `tests/parallel_errors.rs` (two failing instruments and a healthy
one refilling together: the same error and the same frames before it on 1, 2, 4
and 8 threads, with mutants that drop the buffered error caught) and the pool's
own tests. Mutants caught: instrument results put
back on the wrong node, a block horizon that ignores the next event. Equivalent:
removing the outer `needs_refill` check (`prefill` checks again).

Review fixes: the thread count limited nothing (a batch went to every pool
thread and a voice split ran other instruments' blocks on top of its own); a
panic dropped the instrument that was out on a worker; the helping loop kept a
caller busy with other engines' jobs; the voice scratch was copied into every
checkpoint; the core count was read on every pool call. The tests now count
voice splits and the widest batch (never above the thread count, mutant
caught) and render the first 6 s of Lantern, which has batches of three or
more, on 1, 2, 3, 4 and 8 threads.

Limits: instruments with incoming modulation or a control render a frame at a
time and stay on the calling thread; kit pieces are not separate tasks; the pool
is shared by every engine in the process with no priority, so a chase can delay
the engine feeding the ring when cores are few (a priority queue was tried and
removed: it changed no measurement). A panic in a worker leaves the
engine usable only to report the error (every caller drops the engine after an
error; no test makes a render panic). The block grid of 128 frames moves block
ends on one thread too; 70 plans (seeds and examples) were compared with main
and none differed.

Limits of the work gate (found in review, speed only, never a sample): the
cost hint times the per-voice phase of the last whole block, not the voice
mixing or an instrument's shared graph, and is not scaled by block length
(blocks are 1-128 frames); an instrument whose voices have all ended keeps its
last hint for up to 15 blocks. The gate at its default is covered by a
bit-identity test but not by one that proves it opens; it is measured on
Homeward and Afterlight above.

Removed after review of the results (2026-11-24, owner's call): the voice split
(about 1.5% on Homeward, within the noise, about 300 lines, and the one place
where a closed gate still handed work to other threads) and the urgent and
background queues of the pool (Afterlight's underruns were cured by the work
gate alone; the queues changed nothing measurable). The pool still lets a
caller help with its own pieces only.
