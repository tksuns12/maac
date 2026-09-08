# Project entrypoint and explicit song resource profile

**Status: implemented normative contract; integrated gates, installed
entrypoint acceptance, and full native source-free rendering passed.**
This document defines one discoverable composition entry and a caller-selected,
finite resource allowance for a complete native song. The command forms, limits
APIs, and master gain are implemented. The [delivery report](project-entrypoint-delivery.md)
records measured evidence, including byte-identical full-song retained rendering.
This extension adds no source grammar, manifest, include mechanism, import
semantics, or performance-plan version.

## One composition source of truth

The conventional master source is `main.maac` in the project directory. It has
one `project` declaration, as an existing composition does. It defines the
complete arrangement, instrument instances, routing, timing, and automation
needed to compile and render the song. The file is analogous to a program's
entrypoint: a reader can find what is played and where the final output goes.

Existing explicit filenames remain valid. Authors are not required to rename
every composition or remove previous generated artifacts. For a project that
adopts this convention, generated plans, WAVs, and temporary stem sources are
derived artifacts; `main.maac` and its explicitly declared dependencies are the
authoritative inputs. An external stem-combining script is not required to
render the native composition.

Authors SHOULD place the project, imports, tempo/meter, instrument instances,
final master routing, tracks, and placements near the top, with pattern and
automation data below. This is a recommended organization, not a new execution order.
Existing forward references permit declarations to refer to definitions later
in the same document. There is still exactly one source namespace per document.

Imports remain library-only. A main composition cannot import another
composition to merge projects, tracks, patterns, or placements. Local libraries
retain exact path/hash pins; built-ins retain exact version selection. There
is no automatic include of neighboring files or scan for song parts.

This short example shows the intended layout and an explicit final gain:

```maac
maac 1;
project song {
  score = [0q, 4q]; rate = 48000Hz;
  tempo = &clock; meter = &metre; output = &master:out; tail = 1s;
}
import acoustic { builtin = "std/acoustic/1.0.0"; }
tempo clock { points = [(0q, 120bpm, step)]; }
meter metre { points = [(0q, 4, 4)]; }
node guitar {
  instrument = &acoustic.nylon_guitar;
  config = { voices = 4; };
}
node mix { type = "core.sum/1"; config = { channels = 2; }; }
node master {
  type = "core.gain/1";
  config = { channels = 2; };
  params = { gain = 0.7; };
}
connect guitar_mix { from = &guitar:out; to = &mix:in; }
connect mix_master { from = &mix:out; to = &master:in; }
track guitar_notes { target = &guitar:events; }
place performance { pattern = &phrase; track = &guitar_notes; at = 0q; }

pattern phrase {
  length = 4q;
  note c { at = 0q; dur = 1q; pitch = C4; velocity = 0.6; }
  note e { at = 1q; dur = 1q; pitch = E4; velocity = 0.5; }
  note g { at = 2q; dur = 3/2q; pitch = G4; velocity = 0.6; }
}
curve ending {
  clock = score;
  points = [(0q, 0.7, linear), (1q, 0.5, step)];
}
automation master_ending {
  target = &master.params.gain; curve = &ending; at = 3q;
}
```

## Source entry selection

Only `check`, `compile`, and `build` gain optional `INPUT` and directory input.
Resolve the entry once, before loading its bundle:

| Invocation input | Selected entry | Implicit project root |
| --- | --- | --- |
| Omitted | `cwd/main.maac` | The current working directory |
| Existing directory `DIR` | `DIR/main.maac` | The selected directory `DIR` |
| Explicit file `FILE` | That file, as before | Its existing canonical-file-parent behavior |

Directory selection recognizes a directory path using ordinary filesystem
resolution, including a directory symlink. Its resolved directory becomes the
containment root. A missing path is not guessed to be a directory. Missing
`main.maac`, nonregular entry files, and inaccessible paths fail explicitly.
There is no recursive search, ancestor search, first-match choice, alternative
filename, or implicit creation of an entry.

`--project-root ROOT` explicitly replaces the implicit root in all three forms.
It changes containment, not entry selection: omitted input still selects
`cwd/main.maac`, and directory input still selects `DIR/main.maac`.

For omitted/directory input, choose and retain the directory root **before**
resolving the entry's symlink target. A `main.maac` symlink outside that root
must fail containment; the loader must not silently adopt the target's parent.
An in-root symlink remains supported. An explicit broader `--project-root`
can admit a contained target by deliberate caller choice. Existing capability
directory handles, same-handle bounded reads, reserved built-in namespace
protection, and import/asset containment remain in force.

As in the existing CLI, imports from an entry symlink resolve relative to its
canonical target's declaring-file location. Preserve that interpretation while
retaining the independently selected directory containment root; the logical
`main.maac` alias must not relocate its imports.

Explicit file commands retain their existing filename/root semantics and
results. Newly omitted/directory forms report the selected source filename in
the command result's `input`; no new plan provenance field is needed.

`check` retains library checking when the selected document is a library;
`compile` and `build` continue to require a composition. Conventional composition
organization does not weaken those document-kind rules. A composition `check`
includes compilation, scheduling, and selected resource-budget validation,
including execution work. It is not a syntax-only success path.

`-o` remains required for `compile`, `build`, and `render`; no output filename
is inferred from `main.maac`. Relative output paths remain relative to the
process working directory, not the selected project directory. Existing
`--force` and failure-preserving publication behavior remain unchanged.
`render` still requires a plan file, and `hash` still requires its file input;
neither performs entrypoint discovery.

## Explicit finite song profile

Here, an execution profile selects a resource allowance. It is distinct from
the language conformance profiles in MaaC/1 §1.2 and does not assert additional
language or audio conformance.

`check`, `compile`, `build`, and `render` accept `--profile default|song`.
Omission is exactly `default`; an unknown name is a CLI usage error. Other
commands do not gain a profile option.

| Profile | Maximum conservative execution work |
| --- | --- |
| `default` | 500,000,000 normalized work units |
| `song` | 10,000,000,000 normalized work units |

The song profile changes **only** the execution-work allowance. Every other
source, syntax, plan-byte, event, duration, channel, voice, graph, asset,
allocated-state, and pluck-delay-memory limit remains unchanged. In particular,
48 kHz mono/stereo scope, the 30-minute duration bound, 4 MiB plan limit,
8,388,608 pluck delay-cell bound, and existing scheduling/expansion bounds still
apply. Work estimation retains every existing weight, including 16 per pluck
sample visit and 2402 per pluck initialization; it does not discount silence,
cache hits, or expected overlap.

The audited full composition requires 4,165,619,697 instrument-work units and
explicit `song` selection. It must compile and render natively from the master
composition under that finite allowance. An implementation must not bypass
validation, raise default limits, substitute pre-rendered stems, or add an
unbounded profile to make that command succeed. The final integrated source's
actual work and render result are recorded in the [delivery report](project-entrypoint-delivery.md).

```sh
maac check --profile song
maac compile --profile song -o song.performance.json
maac build . --profile song -o song.wav --format pcm16
maac render song.performance.json --profile song -o retained.wav --format pcm16
```

A source document or plan cannot request, embed, or authorize a higher profile.
There is no `project.profile`, limits field, manifest permission, or serialized
profile in the plan. Unknown source/plan fields continue to fail. The caller
must select the allowance independently at each validation/render boundary.
A retained large plan therefore needs `--profile song` again on `render`; its
origin under a song-profile compile confers no authority on a default renderer.

Work at the selected bound is permitted if all other checks pass; work above
it fails `E_RESOURCE_LIMIT`. Checked arithmetic rejects overflow. The caller
may choose stricter limits through the Rust APIs, but cannot exceed the
10-billion hard execution ceiling or any unchanged hard ceiling. A failed
budget check must occur before DSP allocation/rendering or output publication.
Larger allowances are not assurances about wall-clock speed or audio quality.

## Explicit Rust limits across every boundary

`PlanLimits::default()` retains its existing values. Add `PlanLimits::song()`
returning the same structure with only `max_execution_work` set to
10,000,000,000. No new struct field or plan-wire field is necessary. Preserve
the existing public 500-million default constant; publish the separate
10-billion maximum as `PlanLimits::MAX_SONG_EXECUTION_WORK`. The internal
ceiling for an explicitly supplied execution-work limit becomes that maximum;
other fields retain their previous ceilings. Caller-supplied values above the
hard ceilings are bounded to those ceilings as existing limits handling does.

Add the following consistent explicit-limit entry points. Each accepts a final
`limits: &PlanLimits` argument, except callback helpers which place `limits`
before the callback so the closure remains last. Existing arguments and return
types otherwise match their default counterparts:

| Boundary | Explicit entry points |
| --- | --- |
| Parsed source | `compiler::check_with_limits(document, limits)`, `compile_with_limits(document, limits)` |
| Source bundle | `compiler::check_bundle_with_limits(bundle, limits)`, `compile_bundle_with_limits(bundle, limits)` |
| Plan decode | `Plan::from_json_with_limits(bytes, limits)`, `from_json_str_with_limits(text, limits)` |
| Plan validation | Existing `Plan::validate_with_limits(limits)` |
| Plan encode | `Plan::to_json_with_limits(limits)`, `to_json_string_with_limits(limits)` |
| Prepared DSP | `DspEngine::new_with_limits(plan, limits)` |
| Streaming DSP | `dsp::render_with_limits(plan, limits, callback)`, `render_plan_with_limits(plan, limits, callback)` |
| WAV writer | `export::write_wav_with_limits(sink, plan, format, limits)` |
| WAV path | `export::render_wav_to_path_with_limits(plan, path, format, force, limits)` |
| Root convenience | Compiler `_with_limits` reexports, `maac::load_plan_with_limits(bytes, limits)`, `maac::render_with_limits(plan, limits, callback)` |

Every existing no-limits API remains a default-profile wrapper. This includes
the direct `Deserialize` implementation for `Plan`: a generic Serde decode
does not authorize larger work. Explicit-limit plan decoding must use the same
strict wire schema and validation under the supplied limits without first
forcing a default-profile validation that would reject a legitimate song plan.

Compilation, library resolution, final plan validation, plan encoding, decoding,
engine preparation, and WAV export must consistently propagate the same caller
limits. No inner default wrapper may silently narrow a selected song operation
or silently enlarge a default operation. Byte/rational bounds still apply
before expensive decode/allocation. Compiler APIs remain filesystem-free;
entrypoint selection belongs to the CLI/loader boundary.

A prepared engine retains its caller-approved immutable plan and may reset and
render it again under that prepared allowance. This does not authorize another
independently constructed engine or a serialized plan. Direct open-ended
instrument runtimes keep their current memory/capacity contract; this extension
does not add a per-process or per-sample elapsed-time governor.

## Visible master gain uses the existing processor contract

Implement `core.gain/1` as already specified in
[MaaC/1 §18.2](../MaaC-1-Specification.md#182-coregain1-and-corefader1).
Its public parameter is `gain`, not `level`: finite, dimensionless, nonnegative,
default 1, sample-rate, error range policy, and `output = gain * input` without
smoothing. It requires `config.channels` and exactly one audio input. The
existing implementation scope admits channels 1 or 2 only; a larger positive
channel count remains unsupported. This does not implement `core.fader/1` or
redefine either processor's normative behavior.

Use additive strict plan processor tag `{"kind":"gain","channels":2}` with
the ordinary node parameter map. Missing/invalid channels, unknown processor
fields, negative/nonfinite gain, wrong units, and a `level` alias fail. Existing
version numbers remain unchanged; old renderers may reject this previously
unsupported processor kind. Existing processor algorithms remain unchanged.
Gain may be zero and may exceed 1 if finite; there is no invented upper gain
bound. Nonfinite multiplication fails. PCM16 overload checks remain separate.
The master source must expose its final gain visibly instead of relying on an
external render script's hidden attenuation or normalization.

## Acceptance matrix

These are targeted requirements, not a claim of completed execution:

| Boundary | Required evidence |
| --- | --- |
| Omitted entry | From a directory containing only `main.maac`, check/compile/build select it; missing main fails without ancestor/recursive fallback |
| Directory entry | Relative, absolute, and space-containing directories select exactly their own main; output stays relative to cwd |
| File compatibility | Existing explicit composition filenames and explicit library checks remain valid; compile/build still reject libraries |
| Symlink containment | Omitted/directory main symlink outside selected root fails; contained link passes; explicit wider/narrower roots are honored; explicit-file root behavior remains compatible |
| Unchanged commands | Missing `-o` still fails where required; render/hash keep required file input and reject directory/default discovery |
| Source authority | One project, forward-referenced patterns/automation, explicit imports and visible master gain compile; imported composition, manifest/include syntax, and source profile escalation fail |
| Default budget | Composition check, compile, load, encode, DSP and export reject work above 500 million through existing wrappers; default CLI behavior stays unchanged |
| Song budget | Every explicit-limit boundary accepts an otherwise valid plan between 500 million and 10 billion; at-limit acceptance and over-limit/overflow rejection are tested without requiring a full long render |
| Other bounds | Song mode still rejects oversized plan/source, duration, event/graph/voice/state/pluck-memory/channel/rate violations; tighter caller budgets still apply |
| Retained plan | A large plan compiled under song renders after source removal only when the render caller selects song; no embedded profile/self-elevation, no default Serde bypass |
| Master gain | Mono/stereo unity and zero, a finite gain above 1, sample-accurate automation, exact multiplication, source/plan strict fields, invalid ranges and finite-output enforcement |
| Native complete song | Master entry check/compile/build with song profile and source-free retained render succeed; record actual work, frames, stereo finite output, headroom, and same-host repeatability; no stem reconstruction is required |
| Existing audio | Existing default API/CLI examples, source pins, plan versions, and old processor sample behavior remain unchanged |

Default rejection of a larger complete composition is an expected resource
diagnostic, not evidence that the composition is semantically invalid. A passing
song-profile check is budget-aware validation; only an observed render proves
audio delivery. Listening judgments remain separate from those numerical checks.
