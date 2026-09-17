# Interchange loss reports

`maac::interchange` implements the first MaaC/1 §25 adapter contract. Every
adapter result carries a versioned machine-readable report rather than silently
approximating source semantics.

The v1 report discriminator is `maac.interchange-loss-report`, version `1`.
Each loss contains a stable `code`, one or more `source_paths`, the affected
`property`, the target `output_limitation`, and a `decision` describing either
an `approximation` or an `omission`. The envelope also records the exact
`adapter_id` and `target_profile`.

Report decoding rejects duplicate JSON keys, floating-point JSON numbers,
unknown fields, unsupported versions, empty required fields, and resource-limit
violations. Reports are limited to 4 MiB and 4096 loss entries.

## Faithful mode

`AdapterPolicy::faithful()` refuses an export if any generated loss code is not
explicitly approved by the caller. The rejection carries the complete proposed
loss report so a host can present the exact decisions for approval. Approval is
by stable loss code and never silently changes the source document.

## Initial MIDI adapter

`export_midi1_smf` targets exactly
`MIDI 1.0 / Standard MIDI File 1.0 format 0 / SMPTE -25 fps, 40 ticks/frame`. The SMPTE division gives a fixed
1000-tick-per-second timeline, avoiding a false claim that continuous MaaC
tempo ramps are natively represented by SMF tempo metadata.

The adapter reports, when relevant:

- `midi.microtonal_pitch` for semitone rounding;
- `midi.zero_velocity_note` when a silent MaaC note would otherwise become a MIDI note-off;
- `midi.overlapping_same_key_identity` for ambiguous same-channel/key overlap;
- `midi.per_note_expression` for omitted MaaC per-note expression;
- `midi.tempo_ramp_discretization` when continuous tempo metadata is omitted;
- `midi.timing_resolution` for frame boundaries rounded to the 1 ms timeline;
- `midi.audio_omission` for recorded/audio graph content;
- `midi.synthesis_routing_omission` for processor and routing state;
- `midi.hit_mapping` when no explicit drum-note mapping exists;
- `midi.message_protocol` for transport messages outside the supported MIDI 1.0
  channel-message subset.

Exact `midi1` channel messages are retained. Note events are emitted on channel
1. This initial adapter deliberately does not invent MPE allocation, General
MIDI drum mappings, synthesis patches, or DAW routing state.
