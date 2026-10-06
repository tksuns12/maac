#!/usr/bin/env python3
"""Render std/basic and std/studio auditions on the same notes, matched in loudness.

Each audition plays one "Late Window" part, or a velocity ladder, through the
std/basic instrument the track used and through the std/studio instrument
meant to replace it. Both renders are measured with `maac analyze` and scaled
to the same integrated loudness, then joined into one A/B file: std/basic
first, a second of silence, then std/studio.

`--calibrate` instead measures each std/studio reference phrase at velocity
0.7: the library's level defaults put them at -20 LUFS. Standard library
only; outputs go below target/studio-auditions.
"""

from __future__ import annotations

import argparse
import json
import math
import struct
import subprocess
import sys
from fractions import Fraction
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET_LUFS = -20.0
RATE = 48_000

HEADER = """maac 1;
project audition {{ score = [0q, {length}q]; rate = 48000Hz; tail = 2s; tempo = &clock; meter = &metre; output = &out:out; }}
tempo clock {{ points = [(0q, 90bpm, step)]; }}
meter metre {{ points = [(0q, 4, 4)]; }}
groove lazy {{ grid = 1/2q; ratio = 2/3; }}
import basic {{ builtin = "std/basic/1.1.0"; }}
import studio {{ builtin = "std/studio/1.0.0"; }}
"""

# "Late Window" comping: a chord on beat 1 and a pushed chord on the and of 3.
COMPING = [
    ("fmaj9", ["A3", "C4", "E4", "G4"]),
    ("em7", ["G3", "B3", "D4", "E4"]),
    ("dm9", ["F3", "A3", "C4", "E4"]),
    ("cmaj9", ["E3", "G3", "B3", "D4"]),
]


def q(value: Fraction | int) -> str:
    """An exact score position or duration in quarter notes."""
    value = Fraction(value)
    return f"{value.numerator}q" if value.denominator == 1 else f"{value.numerator}/{value.denominator}q"


def comping() -> str:
    lines = []
    for bar, (name, pitches) in enumerate(COMPING):
        start = Fraction(bar * 4)
        joined = ", ".join(pitches)
        lines.append(
            f"chord {name}_down {{ at = {q(start)}; dur = 13/10q; pitches = [{joined}]; velocity = [0.5, 0.45, 0.45, 0.42]; }}"
        )
        lines.append(
            f"chord {name}_push {{ at = {q(start + Fraction(5, 2))}; dur = 13/10q; pitches = [{joined}]; velocity = [0.4, 0.36, 0.36, 0.34]; }}"
        )
    return "\n  ".join(lines)


def ladder(pitches: list[str], velocities: list[float]) -> str:
    """Each pitch at each velocity, one per beat, held for three quarters of it."""
    lines = []
    index = 0
    for pitch in pitches:
        for velocity in velocities:
            lines.append(
                f"note n{index} {{ at = {q(index)}; dur = 3/4q; pitch = {pitch}; velocity = {velocity}; }}"
            )
            index += 1
    return "\n  ".join(lines)


BASS_LINE = """note f1 { at = 0q; dur = 19/16q; pitch = F2; velocity = 0.62; }
  note f2 { at = 5/2q; dur = 3/4q; pitch = C3; velocity = 0.48; }
  note f3 { at = 7/2q; dur = 1/2q; pitch = F2; velocity = 0.4; }
  note e1 { at = 4q; dur = 19/16q; pitch = E2; velocity = 0.6; }
  note e2 { at = 13/2q; dur = 3/4q; pitch = B2; velocity = 0.46; }
  note e3 { at = 15/2q; dur = 1/2q; pitch = E2; velocity = 0.4; }
  note d1 { at = 8q; dur = 19/16q; pitch = D2; velocity = 0.62; }
  note d2 { at = 21/2q; dur = 3/4q; pitch = A2; velocity = 0.48; }
  note d3 { at = 23/2q; dur = 1/2q; pitch = D2; velocity = 0.4; }
  note c1 { at = 12q; dur = 19/16q; pitch = C2; velocity = 0.6; }
  note c2 { at = 29/2q; dur = 3/4q; pitch = G2; velocity = 0.46; }
  note c3 { at = 31/2q; dur = 1/2q; pitch = E2; velocity = 0.42; }"""

PAD_CHORDS = """chord d { at = 0q; dur = 4q; pitches = [D3, F3, A3]; velocity = [0.32, 0.3, 0.3]; }
  chord c { at = 4q; dur = 4q; pitches = [C3, E3, G3]; velocity = [0.32, 0.3, 0.3]; }
  chord bb { at = 8q; dur = 4q; pitches = [Bb2, D3, F3]; velocity = [0.32, 0.3, 0.3]; }
  chord bbsus { at = 12q; dur = 4q; pitches = [Bb2, C3, F3]; velocity = [0.32, 0.3, 0.3]; }"""

HATS = [0.5, 0.3, 0.46, 0.3, 0.5, 0.3, 0.46, 0.34]
KICKS = [("0q", 0.85), ("7/4q", 0.5), ("5/2q", 0.72)]


def beat(snare_as_hit: bool) -> str:
    lines = []
    for index, velocity in enumerate(HATS):
        lines.append(f'hit h{index} {{ at = {q(Fraction(index, 2))}; key = "hat"; velocity = {velocity}; }}')
    for index, (at, velocity) in enumerate(KICKS):
        lines.append(f'hit k{index} {{ at = {at}; key = "kick"; velocity = {velocity}; }}')
    if snare_as_hit:
        lines.append('hit s1 { at = 1q; key = "snare"; velocity = 0.7; }')
        lines.append('hit s2 { at = 3q; key = "snare"; velocity = 0.74; }')
    return "\n  ".join(lines)


SNARE_NOTES = """note s1 { at = 1q; dur = 3/20q; pitch = C2; velocity = 0.7; }
  note s2 { at = 3q; dur = 3/20q; pitch = C2; velocity = 0.74; }"""

PIECES = ["kick", "snare", "clap", "hat", "open_hat", "low_tom", "high_tom", "crash"]


def pieces_ladder() -> str:
    lines = []
    index = 0
    for key in PIECES:
        for velocity in (0.35, 1.0):
            lines.append(f'hit p{index} {{ at = {q(index)}; key = "{key}"; velocity = {velocity}; }}')
            index += 1
    return "\n  ".join(lines)


# The std/basic settings "Late Window" used, so the A side is what the owner
# heard; levels do not matter because both sides are loudness-matched.
LATE_WINDOW_PARAMS = {
    "electric_piano": "brightness = 2400Hz; release = 600ms;",
    "finger_bass": "brightness = 1800Hz; release = 150ms;",
    "warm_pad": "brightness = 2500Hz; release = 1500ms;",
}


def melodic(name: str, basic: str, studio: str, body: str, length: int, groove: bool) -> dict:
    params = LATE_WINDOW_PARAMS[basic]
    return {
        "name": name,
        "length": length,
        "variants": {
            "basic": {
                "nodes": f"node out {{ instrument = &basic.{basic}; config = {{ voices = 24; }}; params = {{ {params} }}; }}",
                "body": body,
            },
            "studio": {
                "nodes": f"node out {{ instrument = &studio.{studio}; config = {{ voices = 24; }}; }}",
                "body": body,
            },
        },
        "groove": groove,
    }


def auditions() -> list[dict]:
    keys_ladder = ladder(["C3", "C4", "C5"], [0.25, 0.5, 0.8, 1.0])
    bass_ladder = ladder(["E1", "A1", "E2", "A2"], [0.25, 0.5, 0.8, 1.0])
    # "Late Window" plays kick and hats from the basic kit and the snare from
    # its own node.
    drums_basic = {
        "nodes": (
            "node kit { instrument = &basic.drums; params = { kick_level = 0.22; hat_level = 0.4; hat_pan = 0.35; hat_brightness = 5500Hz; }; }\n"
            "node snare { instrument = &basic.snare; config = { voices = 4; }; params = { level = 0.28; pan = 0.05; brightness = 4000Hz; }; }\n"
            'node out { type = "core.sum/1"; config = { channels = 2; }; }\n'
            "connect kit_out { from = &kit:out; to = &out:in; }\n"
            "connect snare_out { from = &snare:out; to = &out:in; }\n"
            "track snare_notes { target = &snare:events; }\n"
            "pattern snare_bar { length = 4q;\n  " + SNARE_NOTES + "\n}\n"
            "place snare_main { pattern = &snare_bar; track = &snare_notes; at = 0q; count = 4; }"
        ),
        "body": beat(False),
        "target": "kit",
    }
    return [
        melodic("keys", "electric_piano", "electric_piano", comping(), 16, True),
        melodic("keys_dynamics", "electric_piano", "electric_piano", keys_ladder, 12, False),
        melodic("bass", "finger_bass", "bass", BASS_LINE, 16, True),
        melodic("bass_dynamics", "finger_bass", "bass", bass_ladder, 16, False),
        melodic("pad", "warm_pad", "pad", PAD_CHORDS, 16, True),
        {
            "name": "drums",
            "length": 16,
            "variants": {
                "basic": drums_basic,
                "studio": {"nodes": "node out { instrument = &studio.drums; }", "body": beat(True)},
            },
            "groove": True,
            "count": 4,
        },
        {
            "name": "drum_pieces",
            "length": 16,
            "variants": {
                "basic": {"nodes": "node out { instrument = &basic.drums; }", "body": pieces_ladder()},
                "studio": {"nodes": "node out { instrument = &studio.drums; }", "body": pieces_ladder()},
            },
            "groove": False,
        },
    ]


def at_velocity(body: str, velocity: str) -> str:
    """The same events with every velocity, or velocity list, replaced."""
    import re

    return re.sub(r"velocity = (\[[^\]]*\]|[0-9.]+);", f"velocity = {velocity};", body)


# Calibration targets for the kit pieces relative to the kick, in LU, each
# measured alone on one hit per beat: a boom-bap balance with the hats well
# under the kick and snare.
PIECE_BALANCE = {
    "kick": 0.0,
    "snare": 0.0,
    "clap": -2.0,
    "hat": -10.0,
    "open_hat": -8.0,
    "low_tom": -1.0,
    "high_tom": -1.0,
    "crash": -6.0,
}


def references() -> list[dict]:
    """Reference phrases at velocity 0.7 for level calibration (F15)."""
    def single(name: str, export: str, body: str, groove: bool) -> dict:
        return {
            "name": name,
            "length": 16,
            "variants": {"studio": {"nodes": f"node out {{ instrument = &studio.{export}; config = {{ voices = 24; }}; }}", "body": at_velocity(body, "0.7")}},
            "groove": groove,
        }

    phrases = [
        single("electric_piano", "electric_piano", comping(), True),
        single("bass", "bass", BASS_LINE, True),
        single("pad", "pad", PAD_CHORDS, True),
        {
            "name": "drums",
            "length": 16,
            "variants": {"studio": {"nodes": "node out { instrument = &studio.drums; }", "body": at_velocity(beat(True), "0.7")}},
            "groove": True,
            "count": 4,
        },
    ]
    for key in PIECES:
        hits = "\n  ".join(f'hit b{index} {{ at = {q(index)}; key = "{key}"; velocity = 0.7; }}' for index in range(16))
        phrases.append(
            {
                "name": f"piece:{key}",
                "length": 16,
                "variants": {"studio": {"nodes": "node out { instrument = &studio.drums; }", "body": hits}},
                "groove": False,
            }
        )
    return phrases


def composition(audition: dict, variant: str) -> str:
    spec = audition["variants"][variant]
    count = audition.get("count", 1)
    pattern_length = audition["length"] // count
    groove = " groove = &lazy;" if audition["groove"] else ""
    return (
        HEADER.format(length=audition["length"])
        + spec["nodes"]
        + f"\ntrack notes {{ target = &{spec.get('target', 'out')}:events; }}\n"
        + f"pattern part {{ length = {pattern_length}q;\n  {spec['body']}\n}}\n"
        + f"place main {{ pattern = &part; track = &notes; at = 0q; count = {count};{groove} }}\n"
    )


def run(maac: Path, *args: str) -> str:
    result = subprocess.run(
        [str(maac), *args], cwd=ROOT, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise SystemExit(f"maac {' '.join(args)} failed:\n{result.stderr}{result.stdout}")
    return result.stdout


def read_float_wav(path: Path) -> tuple[int, list[float]]:
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise SystemExit(f"{path} is not a RIFF WAVE file")
    offset, channels, samples = 12, None, None
    while offset + 8 <= len(data):
        chunk, size = data[offset : offset + 4], struct.unpack_from("<I", data, offset + 4)[0]
        body = data[offset + 8 : offset + 8 + size]
        if chunk == b"fmt ":
            fmt, channels, rate, _, _, bits = struct.unpack_from("<HHIIHH", body)
            if fmt not in (3, 0xFFFE) or bits != 32 or rate != RATE:
                raise SystemExit(f"{path} is not 48 kHz float32")
        elif chunk == b"data":
            samples = list(struct.unpack(f"<{size // 4}f", body))
        offset += 8 + size + (size & 1)
    if channels is None or samples is None:
        raise SystemExit(f"{path} lacks fmt or data")
    return channels, samples


def write_float_wav(path: Path, channels: int, samples: list[float]) -> None:
    payload = struct.pack(f"<{len(samples)}f", *samples)
    fmt = struct.pack("<HHIIHH", 3, channels, RATE, RATE * channels * 4, channels * 4, 32)
    path.write_bytes(
        b"RIFF"
        + struct.pack("<I", 4 + 8 + len(fmt) + 8 + len(payload))
        + b"WAVE"
        + b"fmt "
        + struct.pack("<I", len(fmt))
        + fmt
        + b"data"
        + struct.pack("<I", len(payload))
        + payload
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--maac", type=Path, default=ROOT / "target" / "release" / "maac")
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "studio-auditions")
    parser.add_argument("--only", action="append", help="render only the named auditions")
    parser.add_argument(
        "--calibrate",
        action="store_true",
        help="measure each std/studio reference phrase at velocity 0.7 instead of rendering A/B files",
    )
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    if args.calibrate:
        measured = {}
        for phrase in references():
            source = args.out / f"reference-{phrase['name'].replace(':', '-')}.maac"
            source.write_text(composition(phrase, "studio"))
            whole = json.loads(
                run(args.maac, "analyze", str(source), "--project-root", ".", "--profile", "song", "--json", "--section", "whole")
            )["analysis"]["sources"][0]["whole"]
            measured[phrase["name"]] = whole["integrated_lufs"]
            print(f"{phrase['name']:16s} {whole['integrated_lufs']:7.2f} LUFS  true peak {whole['true_peak_dbtp']:6.2f} dBTP")
        kick = measured["piece:kick"]
        for key, offset in PIECE_BALANCE.items():
            print(f"  {key:9s} needs {kick + offset - measured['piece:' + key]:+6.2f} dB to sit {offset:+.0f} LU from the kick")
        (args.out / "calibration.json").write_text(json.dumps(measured, indent=2) + "\n")
        return 0
    report = []
    for audition in auditions():
        if args.only and audition["name"] not in args.only:
            continue
        rendered = {}
        for variant in ("basic", "studio"):
            stem = args.out / f"{audition['name']}-{variant}"
            source = stem.with_suffix(".maac")
            source.write_text(composition(audition, variant))
            wav = stem.with_suffix(".wav")
            run(args.maac, "build", str(source), "--project-root", ".", "--profile", "song", "-o", str(wav), "--force")
            analysis = json.loads(
                run(args.maac, "analyze", str(source), "--project-root", ".", "--profile", "song", "--json", "--section", "whole")
            )["analysis"]["sources"][0]["whole"]
            channels, samples = read_float_wav(wav)
            rendered[variant] = (channels, samples, analysis)
        parts, row = [], {"audition": audition["name"]}
        for variant in ("basic", "studio"):
            channels, samples, analysis = rendered[variant]
            gain = 10 ** ((TARGET_LUFS - analysis["integrated_lufs"]) / 20)
            scaled = [sample * gain for sample in samples]
            if channels == 1:
                scaled = [value for sample in scaled for value in (sample, sample)]
            parts.extend(scaled)
            if variant == "basic":
                parts.extend([0.0] * (2 * RATE))
            peak = max(abs(value) for value in scaled)
            row[variant] = {
                "integrated_lufs": analysis["integrated_lufs"],
                "true_peak_dbtp": analysis["true_peak_dbtp"],
                "matched_peak_dbfs": round(20 * math.log10(peak), 2) if peak else None,
                "bands_db": analysis["bands_db"],
            }
        write_float_wav(args.out / f"{audition['name']}-AB.wav", 2, parts)
        report.append(row)
        basic, studio = row["basic"], row["studio"]
        print(
            f"{audition['name']:14s} basic {basic['integrated_lufs']:7.2f} LUFS  "
            f"studio {studio['integrated_lufs']:7.2f} LUFS  "
            f"matched peaks {basic['matched_peak_dbfs']} / {studio['matched_peak_dbfs']} dBFS"
        )
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"A/B files and report.json in {args.out.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
