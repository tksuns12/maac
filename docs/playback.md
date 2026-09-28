# Rendered playback on macOS

`maac play` auditions a source composition or retained performance plan through
macOS's system-default audio output. It first renders the complete output to a
private Float32 WAV, then runs the system `/usr/bin/afplay` player. Compilation,
asset pins, native DSP, resource profiles, and WAV conversion use the same
existing paths as `build` and `render`.

## Commands

```sh
# Conventional main.maac in the current directory:
maac play

# Explicit source or project directory:
maac play examples/grouped-takes.maac --project-root .
maac play path/to/project --disk-media --profile song

# A retained plan must be selected explicitly:
maac play composition.plan.json --plan --profile song
```

Source mode accepts the same omitted input, directory, explicit source file,
and `--project-root` behavior as `build`. `--disk-media` enables existing private
native PCM snapshots. The renderer verifies every declared dependency before
playback, including inactive take files. The `default` and `song` work profiles
retain their normal limits; playback does not increase them.

`--plan` requires an explicit input and conflicts with `--project-root` and
`--disk-media`. Retained plans are validated and rendered through the existing
standalone artifact path. Input is never inferred from an extension or file
contents. Names beginning with a dash can follow `--`.

The command plays the full 48 kHz mono/stereo output, including the authored
tail. Rendering finishes before sound starts. There are no range, seek, pause,
loop, volume, format, or device-selection flags in this slice. System settings
control output route and volume; macOS may perform device format conversion.
Playback does not alter source, save a WAV beside it, or modify an archive.

## Stop and errors

Press **Ctrl-C** to stop during source loading, compilation, rendering, or
playback. The CLI also handles SIGTERM. It terminates and reaps its active child
before removing the private WAV, partial render files, and captured diagnostics.
Renderer-created media snapshots are contained in the same owner-only staging
directory, including when the renderer is forcibly stopped.
If filesystem permissions prevent cleanup, the error includes the surviving
staging path so it can be removed after access is restored.
It restores the previous signal handlers before returning. A handled SIGINT
returns exit 130; SIGTERM returns 143. Forced process death such as SIGKILL and
power loss cannot run this cleanup.

Rendering and playback run as separate supervised processes. Playback starts
only after successful rendering and validation of the completed WAV. Renderer
failures retain their diagnostic code, source path, and span. Messages larger
than 64 KiB are shortened at a UTF-8 boundary with an explicit `[truncated]`
marker; shorter messages are preserved. Player
startup, player failure, or an invalid child result reports `E_PLAYBACK`;
staging filesystem failures report `E_IO`. Interrupts report `E_INTERRUPTED`.

The initial backend is macOS `/usr/bin/afplay` on the system-default route.
Other platforms return `E_CAPABILITY` before rendering. A missing player or
unavailable output device is an explicit error; MaaC does not silently choose
another executable or device. There is no backend override flag or environment
variable.

## Results and evidence

`--json` emits one terminal JSON object on stdout and leaves stderr empty.
Success identifies `command="play"`, `status="completed"`,
`backend="macos-afplay"`, `output_device="system-default"`, the staged Float32
format, frame count, sample rate, and channel count. It omits the temporary
output path. Human output reports terminal completion or failure.

Success means the player process exited successfully. The frame count describes
the staged WAV; it does not measure frames heard, output-device latency, or the
physical device rate. See the [verification record](verification.md) for the
actual backend smoke result and separately recorded listening evidence.

This is a process CLI command. The public `Command` enum and `execute*` embedding
APIs retain their existing interfaces. The supervisor invokes the same installed
MaaC executable for rendering, so no extra renderer installation is needed.

Staging uses temporary disk space for a complete Float32 WAV, plus the existing
source/media snapshots. At the 30-minute stereo limit the WAV alone is roughly
691 MB. Existing duration, media, graph, and work limits still apply, and disk
errors are reported explicitly.

Bounded [device recording](recording.md) is available separately. Input monitoring,
playback-output selection, latency-aligned overdub, low-latency transport,
and producer listening acceptance remain separate
[roadmap](end-to-end-production-plan.md) gates.
