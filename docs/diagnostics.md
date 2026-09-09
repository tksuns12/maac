# Diagnostics

Commands print human-readable results by default. `--json` selects structured
command results and diagnostics for scripts. Errors use stable codes, object and
field paths, and source byte spans where available. Parser spans are half-open
UTF-8 byte ranges; they refer to the original authored source.

| Code | Meaning and typical correction |
| --- | --- |
| `E_SYNTAX` | Fix an invalid token, delimiter, string escape or value form |
| `E_DUPLICATE_ID`, `E_DUPLICATE_FIELD` | Give identities unique names and specify each field once |
| `E_UNKNOWN_KIND`, `E_UNKNOWN_FIELD` | Check spelling and the normative object/field inventory |
| `E_REFERENCE` | Point to an existing object of the required type |
| `E_ASSET`, `E_HASH` | Supply supported asset metadata and exact bytes matching the declared SHA-256 pin |
| `E_UNIT` | Supply the declared dimension, such as `q`, `ms`, `Hz` or `ct` |
| `E_RANGE`, `E_INTERVAL` | Correct a value, onset, duration or score span |
| `E_TEMPO`, `E_METER_BOUNDARY` | Use a positive ordered tempo map and meter changes at bar boundaries |
| `E_SUBSAMPLE_NOTE` | The positive physical gate collapses to one frame boundary; author a longer gate |
| `E_PATTERN_CYCLE` | Remove recursive pattern references |
| `E_INSTANCE_TARGET` | Correct an occurrence address after a structural edit |
| `E_AUTOMATION_WRITER` | Combine replacement lanes into one curve per parameter |
| `E_CAPABILITY` | The declared feature is outside the foundation capability set |
| `E_PORT_TYPE`, `E_ALGEBRAIC_LOOP` | Correct channel/port/cardinality mismatches or cyclic routing |
| `E_VOICE_LIMIT` | Increase declared capacity within bounds or reduce overlapping notes or one-shot sample lifetimes |
| `E_NONFINITE` | Reduce values that make a DSP or export calculation nonfinite |
| `E_PCM16_RANGE` | Keep raw samples within `[-1,1]` before PCM16 export; samples are never clipped or normalized |
| `E_OUTPUT_EXISTS` | Choose a new destination or pass `--force` to replace an existing file |
| `E_IO`, `E_WAV`, `E_FORMAT` | Correct the destination, permissions, WAV writer state or requested encoding |
| `E_RENDER_STATE`, `E_RENDER_CALLBACK` | Correct an invalid execution graph or renderer/export boundary failure |
| `E_RESOURCE_LIMIT` | Reduce input size, expansion, nesting, arithmetic size or render work |
| `E_VERSION` | Use a supported source or performance-plan version |

The engine never treats an unavailable processor as an oscillator substitute,
steals a voice, extends a collapsed gate, or ignores an unknown field. A plan is
a derived artifact: after editing source, compile it again. Hand-edited imported
plans receive the same structural, scheduling and resource checks as generated
plans.

Labels and comments are not executable instructions. Source commands load pinned
dependencies through the bounded local bundle loader. Compilation uses that
bundle; retained-plan rendering uses embedded assets. Neither executes plug-ins,
hardware adapters, or network requests.

A hit onset must schedule before the score-end frame. If a physically pre-end
onset rounds to that frame, compilation returns `E_INTERVAL`; move the onset
earlier or extend the score. The declared tail allows samples that already
started to finish, and does not admit new hits.

WAV export performs no gain adjustment. Float32 output accepts every finite
binary64 sample representable as binary32 and rejects nonfinite or overflowing
conversions. PCM16 accepts only finite raw samples in `[-1,1]` and quantizes
`round(sample * 32767)`, so `-1` and `+1` map to `-32767` and `+32767`.
Neither encoding normalizes, limits or dithers. Output is rendered into a
same-directory temporary file and published only after WAV finalization; an
error leaves an existing destination unchanged and does not publish a new one.
