# MaaC Studio

A native Rust companion studio GUI for [MaaC (Music as a Code)](../README.md), built with `iced`, `cpal`, and `maac` linked in-process inside this monorepo.

## Features

- **In-process live playback:** opens `.maac` projects with `BackgroundSession` + `Playback` + `cpal` CoreAudio/ALSA output (allocation-free audio callback with 5 ms de-click play/pause fades).
- **Bar grid, snapping, & seamless looping:** `MeterMap`/`TempoMap`-driven bar ruler (`Bar X.Y` readout), bar-snapped click-to-seek (`Alt` for free seek), drag on the ruler (or `Shift`+drag on the piano roll) to set a bar-snapped seamless bit-exact loop, double-click a region to loop it, right-click to clear loop, and `Auto-Follow` viewport scrolling (`L` loop, `S` snap, `F` follow).
- **Drum Hit & Automation Curve Editing:** click or drag drum bars in the bottom `HITS` strip (vertical = velocity, horizontal = timing, double-click = add hit, sidebar = switch kit piece `key` / duplicate / delete / Make Unique), and visualize/edit `automation` curves directly on the timeline automation lane (drag control points vertically, double-click to add points, switch `step`/`linear`/`exponential` shapes, or delete points in the inspector).
- **Live `.maac` Code & Diff Collaboration Panel (`C`):** collapsible bottom split panel showing live `.maac` source code with instant line highlighting for the selected note or mixer/FX node (`Focused` / `Full` modes), a live `Unsaved Diff` tab showing exact minimal `.maac` changes produced by GUI edits (with `Save*` / `Discard`), and an `AI / External Diff` tab that automatically captures external file-watcher edits from AI agents or editors with one-click `Revert AI Edit`.
- **Live Mixer & Node/FX Parameter Editor (`M`):** toggleable DAW mixing console and sidebar `NODE / FX INSPECTOR` exposing every instrument, kit, bus (`core.gain/1`, `core.fader/1`, `core.pan/1`), and production effect (`fx.reverb/1`, `fx.compressor/1`, `fx.limiter/1`, `fx.eq/1`) parameter with logarithmic Hz/time sliders, `AUTO` badges on automated parameters, reset-to-default (`↺`), and atomic `maac::editing` updates that flow live into playback with full Undo/Redo/Save.
- **External DAW Plugins (macOS Audio Unit v2 & CLAP 1.2) + Native Plugin UI + Freeze to `.maac`:** mount installed AUv2 or `.clap` instruments and effects (`+ Inst`, `+ FX`, `+ Master FX`) directly on tracks, mixer nodes, or the master bus with live parameter sliders inside the sidebar inspector, floating native macOS plugin UI windows (`Open UI` / `UI` button — supporting CLAP `clap.gui` Cocoa views, AUv2 `AUCocoaUIBase` custom views, and `CoreAudioKit` `AUGenericView` fallback), and sidecar state persistence (`<song>.maac-rack.json`). Click **Freeze → .maac** to render any plugin-hosted track, insert, or master chain offline into a pinned `frozen/*.pcm` (`pcm_f32le_interleaved/1`) asset wired atomically into the `.maac` source so pure MaaC CLI, CI, and AI agents render bit-identically without the plugin.
- **SoundFont (`.sf2`) & SFZ (`.sfz`) Sample Instrument Importer:** pure-Rust importer (`Import SF2 / SFZ...` in the Mixer header) that parses `.sf2` presets and `.sfz` key/velocity/loop/ADSR regions into portable, 100% bit-identical native MaaC `synth.sample/1` libraries (`imported/<slug>/<slug>.maac` + pinned `.wav` assets) and wires them directly onto a track.
- **Direct piano-roll authoring (`maac::editing`):** click or `⌘`-click/`⌘`-drag (marquee box) to select one or many notes, edit pitch/beat/duration/velocity or batch-transpose/shift/resize/delete in a single atomic transaction, double-click empty space on the piano roll to insert a new note directly into the active track's pattern, and click **Make Unique** (`prepare_materialize_instance`) to split a shared pattern placement into its own independent pattern. All edits validate via `SourceDocument` + `BundleEditContext::checking_compile` (preserving comments and formatting), flow live into playback, support full `Undo` (`Cmd+Z`) / `Redo` (`Cmd+Shift+Z`), and write to disk only on explicit `Save` (`Cmd+S`).
- **Per-track live audio Solo / Mute:** `S` / `M` buttons on every track filter events through the exact synth, effect, and master chain while keeping transport running.
- **Live collaboration with AI & external editors (`G`):** built-in **AI Co-Producer Collaboration Panel** (auto-detecting local `claude`, `agy`, `codex`, and `gemini` CLIs plus one-click `Copy Context`), **Shared Focus & Timeline Review Pins** (`<song>.maac-collab.json`), **Focused Audio Telemetry** (RMS/Peak/Crest/Stereo Width + Low/Mid/High energy balance + `maac analyze` summary attached to every agent turn), **Cyan AI Edit Timeline Highlights** with `Loop Changed`, and **Instant Zero-Stop Before/After A·B Auditioning** (`A: Before` / `B: After` / `Accept` / `Reject`). External file-watcher saves from AI agents also flow into playback without stopping transport and automatically open the A/B review banner.
- **Piano roll, regions, `maac analyze`, & WAV export:** per-track colored piano roll, drum hit strip, declared region ruler, checkpoint frontier indicator, stereo peak meter, asynchronous `maac analyze` loudness/sections/findings sidebar, and one-click `Export WAV` / `Export Loop`.

## Run

From the repository root:

```sh
cargo run --manifest-path studio/Cargo.toml -- examples/hello.maac
cargo run --manifest-path studio/Cargo.toml --release -- examples/ferry/ferry.maac
```

> On macOS with CLT 26.6 + leftover 27.0 SDK, export `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` before running Cargo.
