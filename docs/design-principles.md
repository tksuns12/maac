# MaaC design principles

MaaC describes a finite musical performance as inspectable, typed, addressable
data. Keep a small declarative musical core, reusable libraries, versioned
engine extensions, and external tools for general computation.

These principles guide future design and contribution decisions. They do not
change existing syntax, processor behavior, conformance obligations, or the
implemented capability set. The [language specification](../MaaC-1-Specification.md)
and applicable extension contracts define behavior; the
[capability matrix](capabilities.md) records implementation support.

## Keep shared musical meaning in the core

The core defines notes, musical units and time, finite patterns, arrangement,
expression, typed references, and the contracts for automation and routing.
These concepts need consistent meaning across instruments, editors, compilers,
and renderers. A lean core limits semantic complexity as well as syntax:
reusing generic object syntax does not make a new behavior free of language cost.

All score-to-seconds conversion uses the shared tempo map. Tempo ramps belong
at this boundary because notes, automation, and audio transports must agree on
the same clock. Linear-in-score-time ramps are already specified; implementing
them fills a capability gap without adding syntax. A library may supply reusable
musical data where supported, but cannot redefine the clock's interpretation.

## Put reusable choices in libraries

Instruments, presets, reusable musical material, and sound-specific expression
mappings belong in libraries where supported by their document contracts.
Composing with a library should primarily involve selecting sounds, setting
musical controls, and arranging notes. Detailed synthesis graphs belong behind
the instrument interface, available to instrument authors without being required
knowledge for ordinary composition.

Pitch, gain, timbre, and pressure are shared expression concepts. An instrument
advertises which it accepts; its defined behavior determines the sound response.
For example, an opted-in instrument graph can map pressure to brightness without
adding a brightness rule to every note or changing other instruments.

Current library documents support instruments, presets, and wavetables, not
pattern or tempo exports. Broader reuse is a design direction, not a claim that
those exports already work. Released library versions retain their defined
content and sound behavior; changes require a new version.

## Give new algorithms explicit engine boundaries

Libraries assemble supported primitives. A synthesis or processing algorithm
that those primitives cannot express needs an engine implementation with a
versioned identity and a defined interface. Specify its parameters, units,
ranges, state and reset behavior, timing, resource limits, and compatibility.
Unsupported required capabilities fail explicitly.

A new string model may justify a processor; a new guitar preset ordinarily
belongs in a library. Preserve the existing reference processors and conformance
contracts. Versioned extensions may be built into the executable; modularity does
not require loading arbitrary executable plug-ins.

## Keep general computation in tools

MaaC composition documents remain declarative, with finite pattern expansion,
explicit transformations, and bounded execution. Do not add general-purpose
control flow, recursive user functions, mutable program variables, or arbitrary
code evaluation to composition documents. Defined automation and private DSP
state remain compatible with this boundary.

External tools can provide algorithmic composition, editing, analysis,
visualization, and import/export workflows. Python or another language can
generate MaaC source; the resulting document and identified dependencies carry
the musical meaning. Validation and rendering must not depend on executing the
generator or reconstructing its intent. Authored production settings that affect
sound remain explicit data under the core or an extension contract even when a
tool applies them.

## Evaluate features at the appropriate boundary

For each proposal, identify whether it establishes shared musical meaning,
packages reusable content, introduces an engine algorithm, or provides an
authoring or delivery workflow. A feature may span several of these boundaries;
state each responsibility explicitly.

Before expanding the language, explain why existing library, extension, or tool
contracts cannot meet the need. Record the intended behavior, compatibility,
finite execution and resource bounds, and observable acceptance criteria.
Distinguish new language semantics from implementing an already specified
feature. Preserve a path for tools to inspect and edit the resulting music
without interpreting a general-purpose program.
