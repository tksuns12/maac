#!/usr/bin/env python3
"""Render the draft electric guitar against the T3 inline guitar, matched in loudness.

Each audition plays "Morning Ferry" material, or a velocity ladder, through
the inline `drive_guitar` from examples/ferry/ferry.maac and through the
draft `electric_guitar` in scripts/guitar/electric_guitar.maac. Both renders
are measured with `maac analyze`, scaled to the same integrated loudness and
joined into one A/B file: T3 first, a second of silence, then the draft.

`--measure` instead plays the draft's reference phrase, the "Morning Ferry"
power-chord chorus at velocity 0.7, at its default drive and across `drive`
1 to 16: the default should read -20 LUFS, and the sweep should stay within
2 LU. Standard library only; outputs go below target/guitar-auditions.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

from studio_auditions import RATE, TARGET_LUFS, read_float_wav, run, write_float_wav

ROOT = Path(__file__).resolve().parent.parent
FERRY = ROOT / "examples" / "ferry" / "ferry.maac"
DRAFT = ROOT / "scripts" / "guitar" / "electric_guitar.maac"

HEADER = """maac 1;
project audition {{ score = [0q, {length}q]; rate = 48000Hz; tail = 2s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 72bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
"""


def block(text: str, kind: str, name: str) -> str:
    """A top-level `kind name { ... }` definition, up to its closing brace."""
    match = re.search(rf"^{kind} {name} \{{.*?^\}}\n", text, re.S | re.M)
    if not match:
        raise SystemExit(f"{kind} {name} not found")
    return match.group(0)


def ferry_patterns(*names: str) -> str:
    text = FERRY.read_text()
    return "".join(block(text, "pattern", name) for name in names)


def instruments() -> dict[str, str]:
    return {
        "t3": block(FERRY.read_text(), "instrument", "drive_guitar"),
        "draft": block(DRAFT.read_text(), "instrument", "electric_guitar"),
    }


def ladder(pitches: list[str], velocities: list[float]) -> str:
    lines = []
    index = 0
    for pitch in pitches:
        for velocity in velocities:
            lines.append(
                f"note n{index} {{ at = {index}q; dur = 3/4q; pitch = {pitch}; velocity = {velocity}; }}"
            )
            index += 1
    return "pattern part { length = %dq;\n  %s\n}\n" % (index, "\n  ".join(lines))


def at_velocity(text: str, velocity: str) -> str:
    return re.sub(r"velocity = (\[[^\]]*\]|[0-9.]+);", f"velocity = {velocity};", text)


# The "Morning Ferry" settings for the T3 side, so the A side is what the
# owner approved; levels do not matter because both sides are matched.
AUDITIONS = [
    {
        "name": "clean",
        "length": 16,
        "patterns": lambda: ferry_patterns("arp_g", "arp_em", "arps_verse"),
        "play": "arps_verse",
        "t3": "drive = 1; tone = 3800Hz; release = 150ms;",
        "draft": "drive = 1;",
    },
    {
        "name": "crunch",
        "length": 16,
        "patterns": lambda: ferry_patterns("pc_g", "power_chorus_a"),
        "play": "power_chorus_a",
        "t3": "drive = 7; tone = 3400Hz; release = 80ms;",
        "draft": "drive = 6; release = 80ms;",
    },
    {
        "name": "lead",
        "length": 32,
        "patterns": lambda: ferry_patterns("melody_chorus"),
        "play": "melody_chorus",
        "t3": "drive = 9; tone = 3200Hz; release = 250ms; vibrato_amount = 0.5;",
        "draft": "drive = 12; pickup = 0.3; tone = 3200Hz; release = 250ms; vibrato = 0.5;",
    },
    {
        "name": "dynamics",
        "length": 16,
        "patterns": lambda: ladder(["E2", "E3", "E4", "E5"], [0.25, 0.5, 0.8, 1.0]),
        "play": "part",
        "t3": "drive = 4;",
        "draft": "drive = 4;",
    },
]


def composition(length: int, instrument: str, export: str, params: str, patterns: str, play: str) -> str:
    return (
        HEADER.format(length=length)
        + instrument
        + f"node out {{ instrument = &{export}; config = {{ voices = 24; }}; params = {{ {params} }}; }}\n"
        + patterns
        + "track notes { target = &out:events; }\n"
        + f"place main {{ pattern = &{play}; track = &notes; at = 0q; }}\n"
    )


def analyze(maac: Path, source: Path) -> dict:
    return json.loads(
        run(maac, "analyze", str(source), "--project-root", ".", "--profile", "song", "--json", "--section", "whole")
    )["analysis"]["sources"][0]["whole"]


def measure(maac: Path, out: Path) -> int:
    instrument = instruments()["draft"]
    patterns = at_velocity(ferry_patterns("pc_g", "power_chorus_a"), "0.7")
    measured = {}
    for drive in [None, 1, 2, 4, 8, 12, 16]:
        params = "" if drive is None else f"drive = {drive};"
        label = "default" if drive is None else f"drive-{drive}"
        source = out / f"reference-{label}.maac"
        source.write_text(composition(16, instrument, "electric_guitar", params, patterns, "power_chorus_a"))
        whole = analyze(maac, source)
        measured[label] = whole
        print(f"{label:9s} {whole['integrated_lufs']:7.2f} LUFS  true peak {whole['true_peak_dbtp']:6.2f} dBTP")
    sweep = [measured[f"drive-{d}"]["integrated_lufs"] for d in (1, 2, 4, 8, 12, 16)]
    print(f"drive sweep spans {max(sweep) - min(sweep):.2f} LU")
    (out / "measure.json").write_text(json.dumps(measured, indent=2) + "\n")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--maac", type=Path, default=ROOT / "target" / "release" / "maac")
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "guitar-auditions")
    parser.add_argument("--only", action="append", help="render only the named auditions")
    parser.add_argument("--measure", action="store_true", help="measure the reference phrase and the drive sweep")
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    if args.measure:
        return measure(args.maac, args.out)
    exports = {"t3": "drive_guitar", "draft": "electric_guitar"}
    sources = instruments()
    report = []
    for audition in AUDITIONS:
        if args.only and audition["name"] not in args.only:
            continue
        parts, row = [], {"audition": audition["name"]}
        for variant in ("t3", "draft"):
            stem = args.out / f"{audition['name']}-{variant}"
            source = stem.with_suffix(".maac")
            source.write_text(
                composition(
                    audition["length"],
                    sources[variant],
                    exports[variant],
                    audition[variant],
                    audition["patterns"](),
                    audition["play"],
                )
            )
            wav = stem.with_suffix(".wav")
            run(args.maac, "build", str(source), "--project-root", ".", "--profile", "song", "-o", str(wav), "--force")
            whole = analyze(args.maac, source)
            channels, samples = read_float_wav(wav)
            gain = 10 ** ((TARGET_LUFS - whole["integrated_lufs"]) / 20)
            scaled = [sample * gain for sample in samples]
            if channels == 1:
                scaled = [value for sample in scaled for value in (sample, sample)]
            parts.extend(scaled)
            if variant == "t3":
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
            f"{audition['name']:9s} t3 {row['t3']['integrated_lufs']:7.2f} LUFS  "
            f"draft {row['draft']['integrated_lufs']:7.2f} LUFS  "
            f"matched peaks {row['t3']['matched_peak_dbfs']} / {row['draft']['matched_peak_dbfs']} dBFS"
        )
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"A/B files and report.json in {args.out.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
