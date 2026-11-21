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
drag may not switch until it stops); `read` frees chunk buffers on the audio
thread; a panic in the catch-up's producer side is now stopped by a guard but
untested.
