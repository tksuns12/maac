# Windowed event queries

A resolved performance can be queried over an exact score window without
changing any event. This is the bounded P5 implementation of the expansion
query in [MaaC/1 §10](../MaaC-1-Specification.md#10-tracks-placement-and-event-identity).

```rust
use maac::{compile_bundle_artifact, parse_rational, SourceBundle};

let source = std::fs::read_to_string("main.maac")?;
let bundle = SourceBundle::new("main.maac", source);
let performance = compile_bundle_artifact(&bundle)?;
let start = parse_rational("5/4")?;
let end = parse_rational("3/2")?;
let events = performance.query_events_score_window(&start, &end)?;
```

The CLI provides the same query for source projects and retained plans:

```sh
maac --json query-events song.maac --start-q 5/4 --end-q 3/2
maac --json query-events song.json --plan --start-q 5/4 --end-q 3/2
```

Use `--profile song` when the source or retained plan requires its explicit
execution-work allowance. The command reads input and writes no file.

`start` and `end` are global score coordinates in quarter-note units. The
window is half-open, `[start, end)`, and both endpoints must lie within the
project score. An empty window returns no events. Reversed or out-of-score
windows fail with `E_INTERVAL`.

A note matches when its final score gate overlaps the window:
`score_on_q < end && score_off_q > start`. Hits and messages are point events;
they match when `start <= score_on_q < end`. The final gate incorporates pattern
and placement repetition, stretch, cut boundaries, and occurrence overrides.
Deleted occurrences are absent; inserts are included once at their placement
coordinates. A spilling note may have `score_off_q` after the project score end.

Every result retains its **complete** `score_on_q` and `score_off_q`, structural
event address, source mapping, event kind and payload, target, physical offsets,
scheduled frames, and order. Querying `[5/4q, 3/2q)` for a note with gate
`[0q, 3q)` returns `[0q, 3q)`; it does not crop the note to the window.
Results are ordered by scheduled onset frame, event order, then address bytes.

Membership uses score coordinates. Physical onset/release offsets and the
project-end schedule clamp remain in the returned record but do not move a note
in or out of a score window. Instrument release tails are not part of the note
gate. A host seeking an audible frame-range query needs a separate clock and
tail policy.

The API queries a fully validated `PlanArtifact`, including artifacts loaded
from JSON. Source callers compile the complete composition first, so malformed
source and global expansion/resource limits still fail even if the requested
window is empty or narrow. This implementation does not skip expansion work
outside the window; lazy expansion is a separate optimization with its own
validation and resource design.

`query_events_score_window_with_limits` applies an explicit `PlanLimits`
allowance to the artifact and query coordinates. Both query endpoints must fit
the allowed rational bit length before interval comparison.

## Verification

The fixed [API vectors](../tests/windowed_event_query.rs) cover nested spill,
inherited cut, half-open boundaries, overrides, deletion, inserts, message and
native-hit points, ramp timing, project-end frame clamping, retained-plan
round trips and ordering, invalid windows, rational limits, and invalid
artifacts. The [CLI vectors](../tests/windowed_event_query_cli.rs) cover source
and retained-plan equivalence, exact bounds, read-only behavior, and embedded
result access. The existing [execution-profile corpus](../tests/cli_execution_profiles.rs)
now checks that an over-default-budget composition fails a query under the
default profile, including an empty window, and succeeds under `song` for both
source and retained input.

The P5 scoped gate passed 39 tests across those files and the affected
compiler, foundation, and message suites. Targeted Clippy passed with warnings
denied, Rust formatting passed, and the disposable specification smoke check
passed. An independent code/design review found one embedded CLI result-loss
issue; the final implementation rejects the legacy `execute` path and exposes
the window and events through `execute_artifact`, with a regression test. The
complete repository test suite was not used as P5 acceptance evidence.
