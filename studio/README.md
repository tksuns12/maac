# MaaC Studio

A native Rust companion studio GUI for [MaaC (Music as a Code)](../README.md), built with `iced`, `cpal`, and `maac` linked in-process inside this monorepo.

## Phase 1 MVP Features

- **In-process live playback:** opens `.maac` projects with `BackgroundSession` + `Playback` + `cpal` CoreAudio/ALSA output (allocation-free audio callback).
- **Seamless transport & looping:** play/pause (`Space`), click-to-seek, drag on the ruler (or `Shift`+drag on the piano roll) to set/adjust a seamless bit-exact loop, double-click a region to loop it, right-click to clear loop (`L` toggles loop).
- **Live collaboration with AI & external editors:** watches the project folder for `.maac` / audio asset saves, recompiles in the background, and calls `BackgroundSession::update` with `edit_debounce` so edits flow into playback without stopping transport.
- **Piano roll, regions, and `maac analyze`:** per-track colored piano roll, drum hit strip, declared region ruler, checkpoint frontier indicator, stereo peak meter, and asynchronous `maac analyze` loudness/findings sidebar.

## Run

From the repository root:

```sh
cargo run --manifest-path studio/Cargo.toml -- examples/hello.maac
cargo run --manifest-path studio/Cargo.toml --release -- examples/ferry/ferry.maac
```

> On macOS with CLT 26.6 + leftover 27.0 SDK, export `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` before running Cargo.
