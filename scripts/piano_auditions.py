#!/usr/bin/env python3
"""Render the std/studio piano against the std/studio electric piano, matched in loudness.

Each audition plays the same notes through `studio.electric_piano`, the
other keyboard in the library, and through std/studio/1.1.0's `piano`, read
from its library source. Both renders are measured with `maac analyze`,
scaled to the same integrated loudness and joined into one A/B file: the
electric piano first, a second of silence, then the piano.

`--measure` instead plays the piano's reference phrase, four bars of ballad
accompaniment across three octaves at velocity 0.7, which the default level
should put at -20 LUFS, and prints each note's level and brightness on a
register and velocity ladder. Standard library only; outputs go below
target/piano-auditions.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

from guitar_auditions import analyze, at_velocity, block
from studio_auditions import RATE, read_float_wav, run, write_float_wav

ROOT = Path(__file__).resolve().parent.parent
LIBRARY = ROOT / "stdlib" / "studio" / "1.1.0.maac"
# The A/B files meet here rather than at -20 LUFS: a piano's attacks peak far
# above its loudness and would clip.
MATCH_LUFS = -27.0

HEADER = """maac 1;
project audition {{ score = [0q, {length}q]; rate = 48000Hz; tail = 3s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 72bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
import studio {{ builtin = "std/studio/1.0.0"; }}
"""

# "Morning Ferry" verse chords, G D Em C, as ballad accompaniment: a low
# root with its octave, then the triad broken in eighths above middle C.
BALLAD_BARS = [
    ("G1", "G2", ["B3", "D4", "G4"]),
    ("D2", "D3", ["A3", "D4", "F#4"]),
    ("E2", "E3", ["B3", "E4", "G4"]),
    ("C2", "C3", ["C4", "E4", "G4"]),
]


def ballad() -> str:
    lines = []
    for bar, (root, octave, triad) in enumerate(BALLAD_BARS):
        start = bar * 4
        lines.append(f"chord l{bar} {{ at = {start}q; dur = 4q; pitches = [{root}, {octave}]; velocity = 0.62; }}")
        pattern = [triad[0], triad[1], triad[2], triad[1], triad[0], triad[1], triad[2], triad[1]]
        for step, pitch in enumerate(pattern):
            velocity = 0.55 if step % 2 == 0 else 0.45
            at = f"{start * 2 + step}/2q"
            lines.append(f"note r{bar}_{step} {{ at = {at}; dur = 1/2q; pitch = {pitch}; velocity = {velocity}; }}")
    return "pattern part { length = 16q;\n  " + "\n  ".join(lines) + "\n}\n"


def ladder(pitches: list[str], velocities: list[float], beats: int) -> tuple[str, int]:
    lines = []
    index = 0
    for pitch in pitches:
        for velocity in velocities:
            lines.append(
                f"note n{index} {{ at = {index * beats}q; dur = {beats - 1}q; pitch = {pitch}; velocity = {velocity}; }}"
            )
            index += 1
    length = index * beats
    return "pattern part { length = %dq;\n  %s\n}\n" % (length, "\n  ".join(lines)), length


REGISTERS = ["C1", "C2", "C3", "C4", "C5", "C6", "C7"]
DYNAMICS = (["C2", "C4", "C6"], [0.2, 0.5, 0.8, 1.0])


def auditions() -> list[dict]:
    registers, registers_length = ladder(REGISTERS, [0.6], 3)
    dynamics, dynamics_length = ladder(*DYNAMICS, 2)
    return [
        {"name": "registers", "length": registers_length, "patterns": registers},
        {"name": "dynamics", "length": dynamics_length, "patterns": dynamics},
        {"name": "ballad", "length": 16, "patterns": ballad()},
    ]


def composition(length: int, variant: str, params: str, patterns: str) -> str:
    if variant == "studio":
        instrument = block(LIBRARY.read_text(), "instrument", "piano")
        node = f"node out {{ instrument = &piano; config = {{ voices = 24; }}; params = {{ {params} }}; }}\n"
    else:
        instrument = ""
        node = f"node out {{ instrument = &studio.electric_piano; config = {{ voices = 24; }}; params = {{ {params} }}; }}\n"
    return (
        HEADER.format(length=length)
        + instrument
        + node
        + patterns
        + "track notes { target = &out:events; }\n"
        + "place main { pattern = &part; track = &notes; at = 0q; }\n"
    )


def note_measures(samples: list[float], channels: int, onsets: list[float], span: float) -> list[tuple[float, float]]:
    """Each note's level in dB and brightness, the first difference's equivalent frequency, over `span` seconds."""
    mono = [sum(samples[k * channels : (k + 1) * channels]) / channels for k in range(len(samples) // channels)]
    result = []
    for onset in onsets:
        start = int(onset * RATE)
        segment = mono[start : start + int(span * RATE)]
        energy = sum(value * value for value in segment) / len(segment)
        difference = sum((segment[k + 1] - segment[k]) ** 2 for k in range(len(segment) - 1)) / len(segment)
        result.append((10 * math.log10(energy + 1e-30), RATE / (2 * math.pi) * math.sqrt(difference / (energy + 1e-30))))
    return result


def measure(maac: Path, out: Path) -> int:
    source = out / "reference.maac"
    source.write_text(composition(16, "studio", "", at_velocity(ballad(), "0.7")))
    whole = analyze(maac, source)
    print(f"reference {whole['integrated_lufs']:7.2f} LUFS  true peak {whole['true_peak_dbtp']:6.2f} dBTP  bands {whole['bands_db']}")
    pitches = ["A0", "C2", "C4", "C6", "C8"]
    velocities = [0.2, 0.5, 0.8, 1.0]
    patterns, length = ladder(pitches, velocities, 2)
    source = out / "ladder.maac"
    source.write_text(composition(length, "studio", "", patterns))
    wav = out / "ladder.wav"
    run(maac, "build", str(source), "--project-root", ".", "--profile", "song", "-o", str(wav), "--force")
    channels, samples = read_float_wav(wav)
    beat = 60 / 72
    measures = note_measures(samples, channels, [index * 2 * beat for index in range(len(pitches) * len(velocities))], 0.3)
    for row, pitch in enumerate(pitches):
        cells = measures[row * len(velocities) : (row + 1) * len(velocities)]
        print(f"  {pitch:3s} " + " | ".join(f"{level:6.1f} dB {bright:5.0f} Hz" for level, bright in cells))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--maac", type=Path, default=ROOT / "target" / "release" / "maac")
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "piano-auditions")
    parser.add_argument("--only", action="append", help="render only the named auditions")
    parser.add_argument("--measure", action="store_true", help="measure the reference phrase and a register ladder")
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    if args.measure:
        return measure(args.maac, args.out)
    report = []
    for audition in auditions():
        if args.only and audition["name"] not in args.only:
            continue
        parts, row = [], {"audition": audition["name"]}
        for variant in ("electric_piano", "studio"):
            stem = args.out / f"{audition['name']}-{variant}"
            source = stem.with_suffix(".maac")
            source.write_text(composition(audition["length"], variant, "", audition["patterns"]))
            wav = stem.with_suffix(".wav")
            run(args.maac, "build", str(source), "--project-root", ".", "--profile", "song", "-o", str(wav), "--force")
            whole = analyze(args.maac, source)
            channels, samples = read_float_wav(wav)
            gain = 10 ** ((MATCH_LUFS - whole["integrated_lufs"]) / 20)
            scaled = [sample * gain for sample in samples]
            if channels == 1:
                scaled = [value for sample in scaled for value in (sample, sample)]
            parts.extend(scaled)
            if variant == "electric_piano":
                parts.extend([0.0] * (2 * RATE))
            peak = max(abs(value) for value in scaled)
            row[variant] = {
                "integrated_lufs": whole["integrated_lufs"],
                "true_peak_dbtp": whole["true_peak_dbtp"],
                "matched_peak_dbfs": round(20 * math.log10(peak), 2) if peak else None,
                "bands_db": whole["bands_db"],
            }
        write_float_wav(args.out / f"{audition['name']}-AB.wav", 2, parts)
        report.append(row)
        print(
            f"{audition['name']:10s} electric piano {row['electric_piano']['integrated_lufs']:7.2f} LUFS  "
            f"studio {row['studio']['integrated_lufs']:7.2f} LUFS  "
            f"matched peaks {row['electric_piano']['matched_peak_dbfs']} / {row['studio']['matched_peak_dbfs']} dBFS"
        )
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"A/B files and report.json in {args.out.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
