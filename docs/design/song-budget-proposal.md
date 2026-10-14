# Song work budget: proposal

**Status:** accepted 2026-10-14, when the owner took both recommended
[decisions](#decisions), and implemented; [Results](#results) records the
evidence. This is item 1 of
[what to build next](../reports/ai-production-trial-t5.md#what-to-build-next-in-order)
in the T5 trial: F34, a song work budget that every trial song nearly
fills. It changes a resource policy and one CLI message. The language, the
plan format and every other limit are unchanged.

## Why

Before rendering, `maac` adds up a conservative cost for the whole plan, in
normalized execution-work units, and refuses a plan over its allowance with
`E_RESOURCE_LIMIT`. The default allowance is 500 million units. The `song`
profile, which the caller selects with `--profile song` or
`PlanLimits::song()`, raises it to 10 billion
([execution profiles](../project-entrypoint.md)). The plan cannot raise it
itself.

Every song from the production trials needs the `song` profile, and the
later ones nearly fill it:

| Trial song | Length | Work | Of 10 billion | Units per frame | Longest song at this density |
| --- | ---: | ---: | ---: | ---: | ---: |
| T2 "Late Window" | 1:18 | 3.25 billion | 33 % | 872 | 3:59 |
| T3 "Morning Ferry" | 2:17 | 5.99 billion | 60 % | 908 | 3:49 |
| T4 "Lantern Street" | 2:09 | 8.88 billion | 89 % | 1,436 | 2:25 |
| T5 "Afterlight" | 2:44 | 8.66 billion | 87 % | 1,100 | 3:09 |

The density rose as the songs moved to std/studio's instruments, whose
voices follow velocity through filters and drives, and to synthesizers
defined in the song: T5's supersaw is seven saws per voice. A pop song runs
three to four minutes and a dance track's extended mix five to seven, so at
T4's or T5's density neither can be built. The 10-billion ceiling was set
with the execution profiles, before any of these instruments existed. The
[electric guitar and piano proposal](electric-guitar-piano-proposal.md)
already noted that a piano part might force "a larger song profile".

The cost of the work is time. On an Apple M1 Pro, a full `maac build` of each
song took:

| Trial song | Work | CPU time | Units per second | Audio per second of CPU |
| --- | ---: | ---: | ---: | ---: |
| "Late Window" | 3.25 billion | 11.9 s | 274 million | 6.6 s |
| "Morning Ferry" | 5.99 billion | 27.4 s | 219 million | 5.0 s |
| "Lantern Street" | 8.88 billion | 49.1 s | 181 million | 2.6 s |
| "Afterlight" | 8.66 billion | 68.4 s | 127 million | 2.4 s |

At the slowest of these rates, 10 billion units is 79 s of CPU.

## What the limit is for

The work allowance bounds the compute that a plan can demand before any of
it is spent. A host that renders plans it did not write keeps the default,
and a plan cannot raise its own allowance. The `song` profile is the
caller's explicit choice to spend more. Memory, duration, events, voices,
graph size and the other limits are separate and do not depend on it; the
30-minute duration bound in particular stays.

So the question is how much compute a caller who chose `song` should be
allowed to ask for. Today the answer is about a minute and a half of CPU,
which no longer covers a normal song.

## Proposed contract

### A. A 100-billion song ceiling

`PlanLimits::MAX_SONG_EXECUTION_WORK` becomes 100,000,000,000, and
`PlanLimits::song()` and `--profile song` allow that much. The default stays
500 million. A caller may still pass a lower limit; nothing above the
ceiling is accepted.

- **Why 100 billion.** It matches the delivery `song` profile, which
  already allows 100 billion conversion and analysis units
  (`DeliveryLimits::song()`), so one `--profile song` means one scale on
  both sides. It covers the 30-minute duration bound at T5's density
  (95 billion) and 24 minutes at T4's, the densest trial song.
- **What it costs.** At the slowest measured rate, 100 billion units is
  about 13 minutes of CPU. A three-and-a-half-minute pop song at T4's
  density needs 14.5 billion, about 80 s; a six-minute extended mix at T5's
  needs 19 billion, about 2.5 minutes.

### B. The refusal names the profile

When the default allowance refuses a plan for its work, the CLI says how to
allow it:

```text
E_RESOURCE_LIMIT: instrument sample execution work 8663238072 exceeds the selected limit 500000000 at instruments
hint: --profile song allows up to 100000000000 units
```

With `--json`, the error object gains an optional `hint` string with the
same text. The hint appears only under the default profile, and only for the
execution-work refusal; under `song` the error is unchanged. In T5 the AI
forgot the option once and had to recall which limit it was.

## Rejected alternatives

- **A budget per second of audio.** A ceiling that grows with the render's
  length, such as 3,000 units per frame, bounds the CPU time per second of
  audio rather than per plan. It would refuse a short, dense passage that
  costs little in total, and it changes the rule's shape from a number to a
  formula. A fixed ceiling over the fixed duration bound already bounds the
  total.
- **No ceiling under `song`.** The ceiling is what makes the profile a
  finite promise to the host. Removing it would leave only overflow checks.
- **`song` as the CLI's default.** The default protects whoever renders a
  plan they did not write. The [execution profiles](../project-entrypoint.md)
  chose an explicit option, and the hint (B) removes most of its cost.
- **A third profile for long tracks.** Another name to choose between, with
  no case that 100 billion does not cover.

## Not proposed

- **Faster rendering.** Worth doing on its own merits, but it does not move
  the ceiling, which counts work, not time.
- **New work weights.** The weights per node stay as they are; this
  proposal changes only how much of them `song` allows.
- **Any other limit.** Duration, memory, voices and the rest stay.

## Versioning

No language, plan or wire change: plans keep their formats and versions, and
retained plans still need `--profile song` to render. Plans between 10 and
100 billion units, which `song` refused, are now accepted under it. The JSON
error object gains an optional field. The change is listed under "Changed"
in the changelog.

## Acceptance

1. `PlanLimits::MAX_SONG_EXECUTION_WORK` and `PlanLimits::song()` give
   100,000,000,000; the default stays 500,000,000.
2. A plan at exactly 100 billion units validates under `song`, and one frame
   more is `E_RESOURCE_LIMIT`, with any caller limit above the ceiling
   bounded to it. As now, the test validates without rendering.
3. A plan between 10 and 100 billion units, refused before, validates and
   renders under `song` through `check`, `compile`, `build`, `render` and
   `deliver`.
4. Under the default profile, an execution-work refusal prints the hint in
   human output and carries it as `hint` in JSON; other errors, and the same
   refusal under `song`, carry none.
5. The documents that state the ceiling are updated: capabilities, the
   quickstart, the CLI reference, execution profiles, the performance plan,
   the plucked-string contract and the std/studio rendering cost.
6. A five-minute extended version of "Afterlight", its drops repeated,
   builds under `song`, and its build time is recorded.

## Cost and risk

- **A host that opts in can wait longer.** A caller that selects `song`
  for plans it does not trust can now be asked for up to about 13 minutes
  of CPU instead of 1.5. The profile's description says so; such a caller
  can pass its own lower limit.
- **Small change.** One constant, its tests, a CLI hint and the documents.

## Decisions

1. **The ceiling.** 100 billion (recommended), matching the delivery
   profile and covering the duration bound at T5's density; or 50 billion,
   about 12 minutes at T4's density and 6.5 minutes of CPU.
2. **The hint.** Print it (recommended), or leave the message as it is.

The owner chose 100 billion and the hint, while noting that a JIT renderer
is under consideration for a later production interface in which an AI and
people work together. The ceiling bounds work rather than adding any, so
faster rendering would only make it cheaper.

## Results

- **Ceiling.** `PlanLimits::MAX_SONG_EXECUTION_WORK` and
  `PlanLimits::song()` give 100,000,000,000; the default stays 500,000,000.
  The library test now builds its at-limit plan from fifty instances of 25
  shared visits over 80 million frames, exactly 100 billion: it validates
  under `song`, and one frame more is `E_RESOURCE_LIMIT` even for a caller
  limit of `u64::MAX`.
- **Hint.** Under the default profile the CLI adds the hint when the
  refused work is within the `song` ceiling. It is added where the binary
  prints the error, so `CliError` and the Rust `cli::execute` API are
  unchanged. A new CLI test checks the human
  line and the JSON `hint` field, and that no hint appears for work beyond
  the ceiling (184 billion units, under either profile) or for a graph limit
  that `song` leaves unchanged.
- **A five-minute track.** "Afterlight" placed twice in a row, 5:24 with
  every placement repeated (its automations are not), needs 17.2 billion
  units, which the old ceiling refused. Under `song` it now builds in 134 s
  of CPU (150 s wall clock) on the M1 Pro.
- **Documents.** The capabilities matrix, quickstart, CLI reference,
  execution-profile contract, performance plan, plucked-string contract,
  std/studio rendering cost and AI producer guide state the new ceiling;
  the execution-profile contract records the change and the hint.
