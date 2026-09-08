#!/usr/bin/env python3
"""Generate the deterministic three-frame starter wavetable.

The waves are authored Fourier sums. Their declared amplitudes are preserved:
there is no peak normalization, resampling, or inferred cycle processing.
"""

import math
import struct
import wave
from pathlib import Path


CYCLE_LENGTH = 512
RATE_METADATA = 48_000
FRAMES = (
    ((1, 0.42),),
    ((1, 0.34), (2, 0.16), (3, 0.08)),
    tuple((harmonic, 0.30 / harmonic) for harmonic in range(1, 9)),
)


def sample(frame: tuple[tuple[int, float], ...], index: int) -> float:
    phase = 2.0 * math.pi * index / CYCLE_LENGTH
    return sum(amplitude * math.sin(harmonic * phase) for harmonic, amplitude in frame)


def main() -> None:
    destination = Path(__file__).with_name("colors.wav")
    with wave.open(str(destination), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(RATE_METADATA)
        for frame in FRAMES:
            for index in range(CYCLE_LENGTH):
                value = max(-1.0, min(1.0, sample(frame, index)))
                output.writeframesraw(struct.pack("<h", round(value * 32767.0)))


if __name__ == "__main__":
    main()
