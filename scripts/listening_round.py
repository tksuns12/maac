#!/usr/bin/env python3
"""Render one listening file per built-in instrument for an approval round.

Each file plays a musical phrase from a MaaC song, a second and a half of
silence, then a ladder of low, middle and high notes from soft to hard. The
phrase is scaled to -20 LUFS integrated and the ladder by the same gain, so
the ladder keeps its dynamics; a file that would peak above -1 dBFS comes
down as a whole. `--set studio` (the default) renders the std/studio/1.0.0
exports and the std/acoustic/1.0.0 guitars; `--set basic` renders the
std/basic/1.1.0 instruments, with its eight drums heard through its kit.
Standard library only; outputs go below target/listening-round.
"""

from __future__ import annotations

import argparse
import math
import re
import sys
from pathlib import Path

from guitar_auditions import analyze, ferry_patterns
from studio_auditions import (
    BASS_LINE,
    PAD_CHORDS,
    RATE,
    beat,
    comping,
    pieces_ladder,
    read_float_wav,
    run,
    write_float_wav,
)

ROOT = Path(__file__).resolve().parent.parent
TARGET_LUFS = -20.0

HEADER = """maac 1;
project listening {{ score = [0q, {length}q]; rate = 48000Hz; tail = 3s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, {bpm}bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
import studio {{ builtin = "std/studio/1.0.0"; }}
import acoustic {{ builtin = "std/acoustic/1.0.0"; }}
import basic {{ builtin = "std/basic/1.1.0"; }}
"""


def ladder(pitches: list[str], velocities: list[float], hit_key: str | None = None) -> tuple[str, int]:
    """Each pitch, or each hit, at each velocity, one per beat."""
    lines = []
    index = 0
    for pitch in pitches:
        for velocity in velocities:
            if hit_key:
                lines.append(f'hit n{index} {{ at = {index}q; key = "{pitch}"; velocity = {velocity}; }}')
            else:
                lines.append(f"note n{index} {{ at = {index}q; dur = 3/4q; pitch = {pitch}; velocity = {velocity}; }}")
            index += 1
    return "pattern part { length = %dq;\n  %s\n}\n" % (index, "\n  ".join(lines)), index


def single(body: str, length: int) -> str:
    return "pattern part { length = %dq;\n  %s\n}\n" % (length, body)


VELOCITIES = [0.3, 0.6, 1.0]

# Name, instrument reference, tempo, phrase (patterns, play, length, groove),
# ladder (patterns, length).
INSTRUMENTS = [
    ("studio-electric-piano", "studio.electric_piano", 90,
     (single(comping(), 16), "part", 16, True), ladder(["C3", "C4", "C5"], VELOCITIES)),
    ("studio-bass", "studio.bass", 90,
     (single(BASS_LINE, 16), "part", 16, True), ladder(["E1", "A1", "E2", "A2"], VELOCITIES)),
    ("studio-pad", "studio.pad", 90,
     (single(PAD_CHORDS, 16), "part", 16, True), ladder(["C3", "C4", "C5"], VELOCITIES)),
    ("studio-drums", "studio.drums", 90,
     (single(beat(True), 4), "part", 4, True),
     (single(pieces_ladder(), 16), 16)),
    ("acoustic-steel-guitar", "acoustic.steel_guitar", 72,
     None, ladder(["E2", "E3", "E4"], VELOCITIES)),
    ("acoustic-nylon-guitar", "acoustic.nylon_guitar", 72,
     None, ladder(["E2", "E3", "E4"], VELOCITIES)),
    ("acoustic-muted-guitar", "acoustic.muted_guitar", 72,
     None, ladder(["E2", "E3", "E4"], VELOCITIES)),
]

def ferry_melody(length: int) -> str:
    """The first `length` quarter notes of the "Morning Ferry" verse melody."""
    kept = []
    for line in ferry_patterns("melody_verse").splitlines():
        at = re.search(r"at = (\d+)(?:/(\d+))?q;", line)
        if at and int(at.group(1)) / int(at.group(2) or 1) < length:
            kept.append(line.strip())
    return "\n  ".join(kept)


KEYS_LADDER = ladder(["C3", "C4", "C5"], VELOCITIES)
BASS_LADDER = ladder(["E1", "A1", "E2", "A2"], VELOCITIES)
GUITAR_LADDER = ladder(["E2", "E3", "E4"], VELOCITIES)
LEAD_LADDER = ladder(["C4", "C5", "C6"], VELOCITIES)

# The std/basic/1.1.0 instruments. Its eight drums (kick, snare, clap,
# closed_hat, open_hat, low_tom, high_tom, crash) are the kit's pieces, so
# the kit's file plays them all.
BASIC = [
    *[(f"basic-{name.replace('_', '-')}", f"basic.{name}", 90,
       (single(comping(), 16), "part", 16, True), KEYS_LADDER)
      for name in ("mellow_piano", "bright_piano", "electric_piano", "organ")],
    *[(f"basic-{name.replace('_', '-')}", f"basic.{name}", 72, None, GUITAR_LADDER)
      for name in ("nylon_guitar", "steel_guitar", "muted_guitar")],
    *[(f"basic-{name.replace('_', '-')}", f"basic.{name}", 90,
       (single(BASS_LINE, 16), "part", 16, True), BASS_LADDER)
      for name in ("finger_bass", "pick_bass", "sub_bass", "synth_bass")],
    *[(f"basic-{name.replace('_', '-')}", f"basic.{name}", 90,
       (single(PAD_CHORDS, 16), "part", 16, True), KEYS_LADDER)
      for name in ("strings", "warm_pad")],
    *[(f"basic-{name.replace('_', '-')}", f"basic.{name}", 72,
       (single(ferry_melody(16), 16), "part", 16, False), LEAD_LADDER)
      for name in ("flute", "synth_lead", "bell")],
    ("basic-drums", "basic.drums", 90,
     (single(beat(True), 4), "part", 4, True),
     (single(pieces_ladder(), 16), 16)),
]

# The guitars play the "Morning Ferry" verse picking and its closing strum.
FERRY_PHRASE = ("arp_g", "arp_em", "arps_verse", "st_g_end")


def guitar_phrase() -> tuple[str, str, int, bool]:
    patterns = ferry_patterns(*FERRY_PHRASE)
    patterns += "pattern part { length = 20q;\n  use verse { pattern = &arps_verse; at = 0q; }\n  use end { pattern = &st_g_end; at = 16q; }\n}\n"
    return patterns, "part", 20, False


def composition(instrument: str, bpm: int, patterns: str, play: str, length: int, groove: bool, count: int = 1) -> str:
    groove_field = " groove = &lazy;" if groove else ""
    return (
        HEADER.format(length=length * count, bpm=bpm)
        # A kit sets each piece's voices itself and takes no config.
        + (f"node out {{ instrument = &{instrument}; }}\n" if instrument.endswith(".drums")
           else f"node out {{ instrument = &{instrument}; config = {{ voices = 24; }}; }}\n")
        + patterns
        + "track notes { target = &out:events; }\n"
        + f"place main {{ pattern = &{play}; track = &notes; at = 0q; count = {count};{groove_field} }}\n"
    )


def render(maac: Path, source: Path, text: str) -> tuple[int, list[float], dict]:
    source.write_text(text)
    wav = source.with_suffix(".wav")
    run(maac, "build", str(source), "--project-root", ".", "--profile", "song", "-o", str(wav), "--force")
    channels, samples = read_float_wav(wav)
    return channels, samples, analyze(maac, source)


def stereo(channels: int, samples: list[float]) -> list[float]:
    return samples if channels == 2 else [value for sample in samples for value in (sample, sample)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--maac", type=Path, default=ROOT / "target" / "release" / "maac")
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "listening-round")
    parser.add_argument("--set", choices=["studio", "basic"], default="studio")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, instrument, bpm, phrase, (ladder_patterns, ladder_length) in (
        BASIC if args.set == "basic" else INSTRUMENTS
    ):
        patterns, play, length, groove = phrase if phrase else guitar_phrase()
        count = 4 if name.endswith("-drums") else 1
        channels, samples, whole = render(
            args.maac, args.out / f"{name}-phrase.maac",
            composition(instrument, bpm, patterns, play, length, groove, count),
        )
        gain = 10 ** ((TARGET_LUFS - whole["integrated_lufs"]) / 20)
        ladder_play = "part"
        ladder_channels, ladder_samples, _ = render(
            args.maac, args.out / f"{name}-ladder.maac",
            composition(instrument, bpm, ladder_patterns, ladder_play, ladder_length, False),
        )
        parts = [value * gain for value in stereo(channels, samples)]
        parts += [0.0] * int(1.5 * RATE) * 2
        parts += [value * gain for value in stereo(ladder_channels, ladder_samples)]
        peak = max(abs(value) for value in parts)
        # A quiet phrase can need enough gain to push the ladder's hard notes
        # past full scale; then the whole file comes down to -1 dBFS peak.
        ceiling = 10 ** (-1 / 20)
        if peak > ceiling:
            parts = [value * ceiling / peak for value in parts]
            gain *= ceiling / peak
            peak = ceiling
        write_float_wav(args.out / f"{name}.wav", 2, parts)
        print(f"{name:24s} phrase {whole['integrated_lufs']:7.2f} LUFS, gain {20 * math.log10(gain):+6.2f} dB, peak {20 * math.log10(peak):6.2f} dBFS")
    print(f"listening files in {args.out.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
