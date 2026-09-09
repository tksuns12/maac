#!/usr/bin/env python3
"""Generate three original drum sounds; no recordings or third-party samples.

Run from any directory with Python 3. Raw output is mono little-endian float32.
The fixed PRNG and formulas reproduce the pinned bytes in this environment;
libm differences across platforms may require checking the printed SHA-256.
"""
import hashlib
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parent


def noise(seed, count):
    for _ in range(count):
        seed = (1664525 * seed + 1013904223) & 0xFFFFFFFF
        yield 2.0 * seed / 0xFFFFFFFF - 1.0


def write(name, rate, samples):
    assert all(math.isfinite(x) and abs(x) < 1 for x in samples)
    raw = struct.pack(f"<{len(samples)}f", *samples)
    (ROOT / f"{name}.pcm").write_bytes(raw)
    print(f"{name}: rate={rate} channels=1 frames={len(samples)} "
          f"bytes={len(raw)} sha256:{hashlib.sha256(raw).hexdigest()}")


def main():
    rate, count = 24000, 4800
    kick = []
    for i in range(count):
        t = i / rate
        phase = 2 * math.pi * (48 * t + 95 * .025 * (1 - math.exp(-t / .025)))
        envelope = min(t / .002, 1) * math.exp(-t / .045) * (1 - i / count)
        kick.append(.48 * envelope * math.sin(phase))
    write("kick", rate, kick)

    rate, count = 24000, 2400
    snare = []
    for i, n in enumerate(noise(0x534E4152, count)):
        t = i / rate
        envelope = min(t / .001, 1) * math.exp(-t / .022) * (1 - i / count)
        snare.append(.30 * envelope * (.75 * n + .25 * math.sin(2 * math.pi * 185 * t)))
    write("snare", rate, snare)

    rate, count = 48000, 1680
    hat, previous = [], 0.0
    for i, n in enumerate(noise(0x48415431, count)):
        t = i / rate
        envelope = min(t / .0003, 1) * math.exp(-t / .007) * (1 - i / count)
        hat.append(.12 * envelope * (n - previous) / 2)
        previous = n
    write("hat", rate, hat)


if __name__ == "__main__":
    main()
