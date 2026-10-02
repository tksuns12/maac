#!/usr/bin/env python3
"""Synthesize the showcase sounds and write showcase.maac with their hashes.

No recordings or third-party samples are used. Run from any directory:

    python3 examples/showcase/generate.py
    maac build examples/showcase/showcase.maac --project-root . -o showcase.wav

WAV samples are embedded in the plan; `.pcm` samples, the phrase and the
stereo glass recording are plan audio assets, so the build also works with
`--disk-media`.
"""
import hashlib
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent
REL = "examples/showcase"
RATE = 48_000
SEED = [0x5EED]


def noise():
    SEED[0] = (1664525 * SEED[0] + 1013904223) & 0xFFFFFFFF
    return 2.0 * SEED[0] / 0xFFFFFFFF - 1.0


def normalize(samples, peak=0.8):
    top = max(abs(x) for x in samples)
    return [x * peak / top for x in samples]


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def interleave(left, right):
    return [x for pair in zip(left, right) for x in pair]


def write_wav(name, samples, channels=1):
    data = struct.pack(f"<{len(samples)}f", *samples)
    fmt = struct.pack("<HHIIHH", 3, channels, RATE, RATE * 4 * channels, 4 * channels, 32)
    raw = (b"RIFF" + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(data)) + b"WAVE"
           + b"fmt " + struct.pack("<I", len(fmt)) + fmt
           + b"data" + struct.pack("<I", len(data)) + data)
    (ROOT / f"{name}.wav").write_bytes(raw)
    return digest(raw)


def write_pcm(name, samples):
    raw = struct.pack(f"<{len(samples)}f", *samples)
    (ROOT / f"{name}.pcm").write_bytes(raw)
    return digest(raw)


def harmonic_tone(period, frames, partials, attack, settle):
    """Partials (h, start, plateau, tau) settle exactly by `settle` frames, so
    any whole number of periods after it loops without a seam."""
    out = []
    for i in range(frames):
        env = min(1.0, i / attack)
        x = 0.0
        for h, start, plateau, tau in partials:
            amp = plateau if i >= settle else plateau + (start - plateau) * math.exp(-i / tau)
            x += amp * math.sin(2 * math.pi * h * i / period)
        out.append(env * x)
    return out


def keys():
    """Electric-piano-like C4 (period 184 = 260.9 Hz) with a bright tine attack."""
    partials = [(1, 1.0, .45, 5000), (2, .6, .12, 3000), (3, .35, .05, 2000),
                (4, .2, .02, 1500), (7, .5, 0, 600), (9, .2, 0, 400)]
    return normalize(harmonic_tone(184, 60_000, partials, 96, 34_000))


def soft():
    """Mellow, nearly sinusoidal C4 with a slow attack."""
    partials = [(1, 1.0, .8, 6000), (2, .15, .1, 4000), (3, .05, .02, 3000)]
    return normalize(harmonic_tone(184, 48_000, partials, 900, 30_000))


def hard():
    """Bright, buzzy C4 with a noisy hammer strike."""
    partials = [(h, 1.6 / h, .7 / h, 2500 + 6000 / h) for h in range(1, 28)]
    tone = harmonic_tone(184, 48_000, partials, 24, 30_000)
    for i in range(1_200):
        tone[i] += 1.2 * noise() * math.exp(-i / 180)
    return normalize(tone)


def low():
    """Bowed, cello-like C3 (period 367 = 130.8 Hz)."""
    partials = [(h, .8 / h, (.8 / h) * (1.3 if h in (3, 4, 5) else 1), 8000) for h in range(1, 36)]
    return normalize(harmonic_tone(367, 48_000, partials, 3_000, 36_000), .7)


def high():
    """Glassy, inharmonic C5 bell that decays on its own (no loop)."""
    base = 523.25
    ratios = [(1, 1.0, 1.1), (2.76, .5, .5), (5.40, .3, .25), (8.93, .15, .12)]
    out = []
    for i in range(48_000):
        t = i / RATE
        env = min(1.0, i / 48)
        out.append(env * sum(a * math.exp(-t / d) * math.sin(2 * math.pi * base * r * t)
                             for r, a, d in ratios))
    return normalize(out)


def stereo_normalize(left, right, peak=0.8):
    top = max(max(abs(x) for x in left), max(abs(x) for x in right))
    return [x * peak / top for x in left], [x * peak / top for x in right]


def wide():
    """Stereo C4 pad. The left channel's period is 185 frames and the right's
    183, so the ears hear 259.5 and 262.3 Hz, with different spectra, evenly
    around the keys sample's 260.9 Hz. Once both settle, 33,855 frames (their
    least common multiple) hold whole periods of each channel, so the loop has
    no seam."""
    left = [(1, 1.0, .7, 2500), (2, .5, .3, 2000), (3, .4, .25, 1500),
            (4, .2, .1, 1500), (5, .15, .08, 1000), (6, .1, .05, 1000)]
    right = [(1, 1.0, .7, 2500), (2, .35, .2, 2000), (3, .5, .3, 1500),
             (4, .15, .08, 1500), (5, .2, .1, 1000), (7, .08, .04, 1000)]
    return stereo_normalize(harmonic_tone(185, 50_000, left, 2_400, 15_000),
                            harmonic_tone(183, 50_000, right, 2_400, 15_000))


def glass():
    """Stereo struck A4 bar, 1.2 s. The bright strike decays quickly on the left
    and a softer ring lasts on the right, 0.5 ms later, so each note starts on
    the left and rings out on the right."""
    f = 440.0
    frames, fade, delay = 57_600, 9_600, 24
    left, right = [], []
    for i in range(frames):
        t = i / RATE
        tail = min(1.0, (frames - i) / fade)
        left.append(min(1.0, i / 24) * tail * (
            math.exp(-t / .18) * math.sin(2 * math.pi * f * t)
            + .5 * math.exp(-t / .06) * math.sin(2 * math.pi * 4 * f * t)
            + .2 * math.exp(-t / .03) * math.sin(2 * math.pi * 10 * f * t)))
        t = (i - delay) / RATE
        right.append(0.0 if i < delay else min(1.0, (i - delay) / 24) * tail * (
            .8 * math.exp(-t / .7) * math.sin(2 * math.pi * f * t)
            + .15 * math.exp(-t / .25) * math.sin(2 * math.pi * 4 * f * t)))
    return stereo_normalize(left, right)


def phrase():
    """A 2.4 s plucked A-minor arpeggio: eight notes, 0.3 s apart."""
    notes = [69, 72, 76, 81, 79, 76, 72, 76]
    out = [0.0] * 115_200
    step = 14_400
    for n, key in enumerate(notes):
        f = 440 * 2 ** ((key - 69) / 12)
        for i in range(step + 9_600):
            j = n * step + i
            if j >= len(out):
                break
            t = i / RATE
            env = min(1.0, i / 48) * math.exp(-t / .22)
            out[j] += env * (math.sin(2 * math.pi * f * t) + .35 * math.sin(4 * math.pi * f * t)
                             + .12 * math.sin(6 * math.pi * f * t))
    return normalize(out, .7)


def note(name, at, dur, key, velocity):
    return f"  note {name} {{ at = {at}q; dur = {dur}q; pitch = key({key}); velocity = {velocity}; }}\n"


def main():
    h = {
        "keys": write_wav("keys", keys()),
        "low": write_wav("low", low()),
        "high": write_wav("high", high()),
        "soft": write_pcm("soft", soft()),
        "hard": write_pcm("hard", hard()),
        "phrase": write_pcm("phrase", phrase()),
        "wide": write_wav("wide", interleave(*wide()), 2),
        "glass": write_pcm("glass", interleave(*glass())),
    }
    for name, value in h.items():
        print(f"{name}: {value}")

    # Section 1 (0-16q): pitched sampler melody, then a held chord on the loop.
    melody = [(0, 60), (1, 64), (2, 67), (3, 72), (4, 71), (5, 67), (6, 64), (7, 62),
              (8, 57), (9, 60), (10, 64), (11, 69), (11.5, 76), (11.75, 79)]
    s1 = "".join(note(f"m{i}", at, "3/4" if at < 11 else "1/4", k, "4/5")
                 for i, (at, k) in enumerate(melody))
    s1 += "".join(note(f"hold{i}", 12, 4, k, "3/5") for i, k in enumerate([48, 60, 64, 67, 71]))

    # Section 2 (16-32q): the same notes at rising velocity cross soft -> hard.
    s2 = "".join(note(f"v{i}", 16 + i, "3/4", 60 if i % 2 == 0 else 67, f"{i + 1}/16")
                 for i in range(16))

    # Section 3 (32-48q): a C-major sweep over the low/high key crossfade.
    scale = [0, 2, 4, 5, 7, 9, 11]
    sweep = [48 + 12 * (d // 7) + scale[d % 7] for d in range(22)]
    s3 = "".join(note(f"k{i}", f"{64 + i}/2", "1/2", k, "3/4") for i, k in enumerate(sweep))
    s3 += note("mid", 44, 2, 66, "3/4")
    s3 += note("bottom", 46, 2, 48, "3/4") + note("top", 46, 2, 84, "3/4")

    # Stereo (79-92q): the glass recording as a sampler, then over the pad.
    tune = [(79, 69, "1/2"), (79.5, 72, "1/2"), (80, 76, "1/2"), (80.5, 79, "1/2"),
            (81, 81, 1), (82, 79, "1/2"), (82.5, 76, "1/2"), (83, 74, "1/2"),
            (83.5, 72, "1/2"), (84, 76, 1), (85, 81, 1), (86, 84, 1), (87, 81, 1),
            (88, 79, 1), (89, 76, "1/2"), (89.5, 79, "1/2"), (90, 74, 1),
            (91, 71, "1/2"), (91.5, 74, "1/2")]
    s4 = "".join(note(f"g{i}", at, dur, k, "4/5") for i, (at, k, dur) in enumerate(tune))
    progression = [[57, 60, 64], [53, 57, 60], [55, 60, 64], [55, 59, 62]]
    pad = "".join(note(f"p{a}_{i}", a, 2, k, "7/10")
                  for a, ks in zip([84, 86, 88, 90], progression) for i, k in enumerate(ks))

    # Finale (93-101q): chords on the keys and the pad, bass on the velocity sampler.
    chords = [(93, [57, 60, 64]), (95, [53, 57, 60]), (97, [55, 60, 64]), (99, [55, 59, 62, 67])]
    s5 = "".join(note(f"c{a}_{i}", a, 2, k, "7/10") for a, ks in chords for i, k in enumerate(ks))
    pad += "".join(note(f"p{a}_{i}", a, 2, k, "2/5") for a, ks in chords for i, k in enumerate(ks))
    bass = "".join(note(f"b{a}", a, "3/2", ks[0] - 12, v)
                   for (a, ks), v in zip(chords, ["1", "2/5", "1", "7/10"]))

    text = (ROOT / "showcase.maac.in").read_text()
    for name, value in h.items():
        text = text.replace(f"@{name.upper()}@", value)
    text = (text.replace("@KEYS_NOTES@", s1 + s5).replace("@LAYER_NOTES@", s2 + bass)
            .replace("@SWEEP_NOTES@", s3).replace("@GLASS_NOTES@", s4)
            .replace("@PAD_NOTES@", pad).replace("@REL@", REL))
    (ROOT / "showcase.maac").write_text(text)


if __name__ == "__main__":
    main()
