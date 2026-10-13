# Showcase listening record

This page records informal user listening approvals. They are separate from
the automated test evidence and do not replace it.

## 2026-10-13: std/basic approval round

| Field | Value |
|---|---|
| Recorded | 2026-10-13 |
| Listener | Project owner (tksuns12) |
| Instruments | The 24 `std/basic/1.1.0` instruments: 16 pitched instruments in a file each, and the eight drums through the `drums` kit, which plays the same drum graphs by hit key |
| Source | [`scripts/listening_round.py`](../../scripts/listening_round.py) `--set basic` at commit `8df9477`, with the release build of `maac` 0.2.0 |
| Files | Made like the 2026-10-12 round's: a phrase, 1.5 s of silence, then a ladder of three or four notes at velocities 0.3, 0.6 and 1. The keys play the "Late Window" comping with a C3–C5 ladder; the basses its bass line, E1–A2; the strings and pad its pad chords, C3–C5; the guitars the "Morning Ferry" verse picking and closing strum, E2–E4; the flute, synth lead and bell the first four bars of its verse melody, C4–C6; the kit a beat, then each drum soft and hard. Each phrase is scaled to −20 LUFS and its ladder by the same gain; no file needed lowering for its peak |
| Playback | Converted to AAC with `afconvert -d aac -b 192000` and sent to an Android phone over the tailnet; whether headphones were used was not stated |
| Verdict | 20 approved, 4 not. On the guitars: "basic-muted-guitar sounds like bass. basic-nylon-guitar too. basic-steel too." On the other 14 files: "다 좋은데 finger base가 뚱뚱뚱 손가락으로 현을 튕기는 느낌이 없네" ("all good, but the finger bass just thuds; there's no feel of a finger plucking a string") |

| File | Length | Verdict | SHA-256 of the WAV | SHA-256 of the AAC heard |
|---|---:|---|---|---|
| `basic-mellow-piano` | 24.2 s | Approved | `859093c83313bbd3f0787ea1e988a4f75f3d25ff09ceffbc35b7110fd74d4ab6` | `a23cf71d415e3e4a40494709f3d154768894fd9aa92c9442d12dcf57a923cc01` |
| `basic-bright-piano` | 24.2 s | Approved | `ab25871322baa6a99b65f2ebd7b4798fc524a2b8c2411f7ed7a2b7fbf2121e67` | `d6b03d22cc817a1b0f8a54faaec16fd6e959176f46cd6502703c5f811569b57e` |
| `basic-electric-piano` | 24.2 s | Approved | `4ea73cfed8e4cff4fdefd9ba5ac894a417bdf6124a05b4bebacb20cb35ede09d` | `ffb13a50370de91af733ed4cc05703a8dd51e6cd0407f8f324631fa1b6e6f3c4` |
| `basic-organ` | 24.2 s | Approved | `d1f63adaef22c2c0d575c0dfed14da9e58668b85ffac9d5188abb6683e761987` | `708976483febb024be8e6509b49af412eba98e40fbf263e69cb234f2c9de9e2a` |
| `basic-nylon-guitar` | 31.7 s | Not approved | `1d849c65669a7094a21a90cac0ad6a8b72261723e8cf9c01126e28967290403c` | `3546b0d405918c50c8821f65c6a8ca038a3baa1da7558be82435bf62ffc20f02` |
| `basic-steel-guitar` | 31.7 s | Not approved | `582e13ee95377c33f3ff69693db91611e1c6caa4f5043a51e40ef29bebea8e17` | `24a562706f824498701f0fa37e162a6ac25085f687162b8c3af2224091f26a7b` |
| `basic-muted-guitar` | 31.7 s | Not approved | `92afa0a79a9d30f761d5e8d5ff0ab4676e9f4289011b09d32ffea7c8f4ba1d67` | `fc1040aadc3c049e380ad0860df0e3485bfffe53eccedbb084a9fd348b7d3153` |
| `basic-finger-bass` | 26.2 s | Not approved | `8569090076a402b83d11e01f4e078a23284700ea668ba64ef27f743e164f6091` | `6c089f548094b40a59f073a70e7cb4a96be7bcb47f94542b00c7aeb8a114cbf8` |
| `basic-pick-bass` | 26.2 s | Approved | `dfb689b2e3f3989228bca4402ca8aaa5930314bbb2db660fa2662a22a16dd27e` | `e67e3d4437b581573ba628c03044cad02f1475e799b48f519fc6a4b36d7f0294` |
| `basic-sub-bass` | 26.2 s | Approved | `ea08fa1717f40a738a885b150660d224d9d156ccc6f99d23ec6bcefcd8c02781` | `071ae51c79d78a7a62bb62c9257150728e13105feb89eec3b7fedb88da0a8bb2` |
| `basic-synth-bass` | 26.2 s | Approved | `0137bcf553aceeb295926bbfa6cc422f7c17166323465b43000711f45cbf5e78` | `dedaa891f21ec571ab2e5eb0d4cdf095d5b3068c5ca5b0ed5d56747d6d844ed3` |
| `basic-strings` | 24.2 s | Approved | `c8e879700191f12f50c7fa7febc74491dedcf2c4eb25c1f014aae33dc6a9a971` | `029838bc1e80b70acc1172c861328502b0a628724493d746c9773ca811403ba0` |
| `basic-warm-pad` | 24.2 s | Approved | `e9f7a2fef0c6290519a705155e9488af75867f1c0c4bcc3834341a89e66c5454` | `324c7f83646fa94c06bf969c27d4f15cccfbc8d42bcc683cecf0ae8831010ee2` |
| `basic-flute` | 28.3 s | Approved | `12b409a15a7206218266d2fdcba3e613fefb55c6538aaf5b0fc265a6e219890e` | `c9f04adb428dd436888147d8c4655f8b6117c372abce06e57a2cf972f37d3436` |
| `basic-synth-lead` | 28.3 s | Approved | `1e1da39c2512daf1b5fd1162fe7a0b6682d45dc17a487232cbde22896c02c919` | `e43a2430272fd98e58648501b144d8dbdc3d58a26d320d404e03969142b9642a` |
| `basic-bell` | 28.3 s | Approved | `e056d2518667034be2db90547a6c3490781802129633222609c222b47505ecc5` | `2c4ab2441d843e20e1a10a7e142204270f18f206876bfcff728e628b8b597a1b` |
| `basic-drums` | 28.8 s | Approved, all eight drums | `01942202031df6eaf1f86efa5e1a960aebb77c857a8b1554a0331238aa585502` | `a28f5413dbeed2532e3fdc918793e3983a6bb802a9b847d7f983ba9af078aa26` |

The guitars came back first. For the other 14 files, the listener was asked
whether each sounds like its name, whether it is usable as it is, and
whether anything bothers. They were also told that a std/basic instrument
changes only its loudness with velocity, by design, and not to count that
against it.

### What the measurements say

- **The guitars carry a bass's spectrum.** Their pitch is right: every note
  `maac analyze` could check is within 5 cents. Compared with the approved
  std/acoustic guitar of the same name, playing the same phrase, each
  std/basic guitar's octave spectrum has 9 to 13 dB more of its energy at
  31.5 to 125 Hz and 8.5 to 24 dB less from 1 kHz up.
- **The finger bass has no pluck.** Its graph is a triangle and a sine
  through a fixed 700 Hz one-pole low-pass, under an 8 ms attack, so nothing
  brightens its onset. One measure of brightness, the RMS-weighted frequency,
  is 0.98 to 1.09 times as high in a ladder note's first 30 ms as 150 to
  300 ms later, and E1's sits at 46 Hz against its 41 Hz fundamental. The
  std/studio `bass`, approved on 2026-10-12, plucks a string model: its
  onsets are 1.30 to 1.78 times as bright, and brighter for harder notes. The
  approved `pick_bass` has no brighter onset either; its square wave through
  1.8 kHz is bright throughout.

std/basic 1.0.0 and 1.1.0 are frozen, so these four sounds stay as they are.
For those parts, use the std/acoustic guitars and the std/studio `bass`. With
this round the owner has heard every built-in instrument, and the same
limits apply as before: one listener on a phone judged each instrument on
its own, without comparing it with other synthesizers or recordings.

## 2026-10-12: built-in instrument approval round

| Field | Value |
|---|---|
| Recorded | 2026-10-12 |
| Listener | Project owner (tksuns12) |
| Instruments | The `std/studio/1.0.0` exports `electric_piano`, `bass`, `pad` and `drums`, and the `std/acoustic/1.0.0` guitars `steel_guitar`, `nylon_guitar` and `muted_guitar`: every built-in instrument outside std/basic not yet approved by ear |
| Source | [`scripts/listening_round.py`](../../scripts/listening_round.py) at commit `142ab84`, with the release build of `maac` 0.2.0 |
| Files | One per instrument: a phrase from "Late Window" (the studio exports) or "Morning Ferry" (the guitars), 1.5 s of silence, then low, middle and high notes at velocities 0.3, 0.6 and 1; for the kit, its beat and then each piece soft and hard. Each phrase is scaled to −20 LUFS and its ladder by the same gain; the muted guitar's file is lowered to a −1 dBFS peak |
| Playback | Sent to an Android phone over the tailnet; whether headphones were used was not stated |
| Verdict | Approved, all seven: "다 졸네", read as "다 좋네" ("all good") |

| File | Length | SHA-256 |
|---|---:|---|
| `listen-studio-electric-piano.wav` | 24.2 s | `6934742a71f0a2ee2593dd6b74c72418f4a9fdbf67bad7ff466b4e72b390763b` |
| `listen-studio-bass.wav` | 26.2 s | `dad12aea18d2c9f593ce8be519e3ca109b4f07d378859ce49ba98bee68c4a70f` |
| `listen-studio-pad.wav` | 24.2 s | `cda1c86aa4a95f7ec3438cc48c94810ba4fa601896c5608fe71c7a81f147fc5c` |
| `listen-studio-drums.wav` | 28.8 s | `dceff64b3986aa40f92ae108cf453c511741b0c4ecf2fafa1803a120534256f1` |
| `listen-acoustic-steel-guitar.wav` | 31.7 s | `1db4ccb2b7fbbc2e001f0898e5c9df4bc8aeab0f81838f077daafb0681d33671` |
| `listen-acoustic-nylon-guitar.wav` | 31.7 s | `826f07f850f39a119064a708be8fa5dfa6725fd6e2d1cc81bbe650ad987a65dc` |
| `listen-acoustic-muted-guitar.wav` | 31.7 s | `3e98206c3754a6a4d72a4d2fb7d046b9025c989b9e1ee584ec018230f6077d01` |

One listener on a phone approved each instrument as it is; the round did
not compare them with other synthesizers or recordings. With the `electric_guitar`
and `piano` approved in their own rounds
([proposal](../design/electric-guitar-piano-proposal.md)), every
`std/studio` and `std/acoustic` instrument has now been approved by ear.
The 24 `std/basic` instruments have not.

## 2026-10-02: stereo showcase

| Field | Value |
|---|---|
| Recorded | 2026-10-02 |
| Listener | Project owner (tksuns12) |
| Source | [`examples/showcase/showcase.maac`](../../examples/showcase/showcase.maac) at commit `aa15157` |
| Build | `maac build examples/showcase/showcase.maac --project-root . --disk-media --profile song -o showcase-stereo.wav --format pcm16` |
| Rendered file | 62.6 s, stereo, 48 kHz, 16-bit PCM WAV, `sha256:71ed6c1ff0670ea5c6ce5e8dbda0c9a9e5c52b66cc4bd01e5dcf773947675aaa` |
| Playback | Downloaded to an Android phone over the tailnet and played there; whether headphones were used was not stated |
| Verdict | Approved: "Great!" |

### What the render adds

- Stereo `synth.sample/1` playback: a stereo WAV pad with a sustain loop, its
  channels tuned to 259.5 and 262.3 Hz.
- One stereo PCM recording declared once as a composition `asset`. It is
  played as an audio clip at its own pitch, then as a pitched sampler through
  `sample glass_a4 { asset = &glass; root = A4; }`.
- The earlier sections are unchanged, copied to both channels through
  `core.matrix/1`. The reverb is stereo, and the master gain is 0.64 dB lower to
  keep the same headroom.

### Later revision (not listened to)

On 2026-10-04 `maac analyze` measured this render's master true peak at
-0.86 dBTP, over the -1 dBTP limit. The excess was only in the finale, where
the keys part was the loudest source. The finale's keys chords were softened
from velocity 7/10 to 1/2. The current render,
`sha256:e58ee5b38e561c3a7c8f895d38e332937b7b5c2807b473d3a3128a2b0932c3c3`,
peaks at -2.31 dBTP:

- bars 1–20 measure exactly as before;
- bar 24, where the finale begins, changes slightly;
- the finale is about 0.9 LU quieter.

The approval above covers the earlier render only; nobody has listened to
the revision yet.

### Scope

This is an overall, informal approval of the whole render. It is not:

- a judgment of the stereo image, which needs headphones;
- a level-matched or blind comparison with the mono render;
- producer acceptance;
- listening evidence for any other example or render.

The numerical checks made when the file was rendered are summarized in the
[examples README](../../examples/README.md#feature-showcase).

## 2026-10-01: mono showcase

| Field | Value |
|---|---|
| Recorded | 2026-10-01 |
| Listener | Project owner (tksuns12) |
| Source | [`examples/showcase/showcase.maac`](../../examples/showcase/showcase.maac) at commit `ce79fc8` |
| Build | `maac build examples/showcase/showcase.maac --project-root . --disk-media --profile song -o showcase.wav --format pcm16` |
| Rendered file | 53.0 s, mono, 48 kHz, 16-bit PCM WAV, `sha256:acbf7fc532115a02b65753706e533a016ab68c8a76ee284071ccee6bde86c4dd` |
| Playback | Downloaded to an Android phone over the tailnet and played there |
| Verdict | Approved: "I liked it" |

### What the render covers

- Pitched `synth.sample/1` playback over two octaves, with a sustain loop on a
  held chord.
- Velocity layers with an equal-power velocity crossfade.
- A key crossfade between two samples.
- Embedded WAV samples and asset-backed PCM samples.
- One recorded phrase played at its own speed, then with `warp_rate` and
  `warp_preserve` (`core.stretch.ola/1`), each at twice and half the speed.

### Scope

This was an overall, informal approval of the whole render on phone playback.
It is not:

- a level-matched or blind comparison;
- a judgment of individual sections, such as the quality of `warp_preserve`;
- producer acceptance;
- listening evidence for any other example or render.

The listener made no specific comments on the stretcher's artifacts.
