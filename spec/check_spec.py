#!/usr/bin/env python3
"""MaaC draft smoke checks, NOT a complete validator or audio renderer.

Requires Python 3.10+, lark, and jsonschema. Run: python check_spec.py
Checks surface syntax, typed syntax-tree shape, example pattern expansion,
selected exact-time rules, curve arithmetic, and the reference noise formula.
"""
from __future__ import annotations

import copy
import hashlib
import json
import math
import re
import struct
from fractions import Fraction
from pathlib import Path
from typing import Any

try:
    from lark import Lark, Transformer
    from jsonschema import Draft202012Validator
except ImportError as exc:
    raise SystemExit("Install the checker's dependencies: pip install lark jsonschema") from exc

ROOT = Path(__file__).resolve().parent
Value = dict[str, Any]


def rational(text: str, tag: str = "number", unit: str | None = None) -> Value:
    x = Fraction(text)
    result: Value = {"t": tag, "n": str(x.numerator), "d": str(x.denominator)}
    if unit is not None:
        result["u"] = unit
    return result


def unique_fields(fields: list[tuple[str, Value]]) -> dict[str, Value]:
    result: dict[str, Value] = {}
    for name, value in fields:
        if name in result:
            raise ValueError(f"E_DUPLICATE_FIELD: {name}")
        result[name] = value
    return result


class ToSyntax(Transformer):
    def number(self, xs: list[Any]) -> Value:
        return rational(str(xs[0]))

    def quantity(self, xs: list[Any]) -> Value:
        return rational(str(xs[0]), "quantity", str(xs[1]))

    def string(self, xs: list[Any]) -> Value:
        text = json.loads(str(xs[0]))
        if any(0xD800 <= ord(ch) <= 0xDFFF for ch in text):
            raise ValueError("E_SYNTAX: unpaired surrogate")
        return {"t": "string", "v": text}

    def symbol(self, xs: list[Any]) -> Value:
        return {"t": "symbol", "v": str(xs[0])}

    def true_value(self, _: list[Any]) -> Value:
        return {"t": "boolean", "v": True}

    def false_value(self, _: list[Any]) -> Value:
        return {"t": "boolean", "v": False}

    def ref_path(self, xs: list[Any]) -> list[str]:
        return [str(x) for x in xs]

    def reference(self, xs: list[Any]) -> Value:
        return {"t": "ref", "path": xs[0], "port": str(xs[1]) if len(xs) > 1 else None}

    def call(self, xs: list[Any]) -> Value:
        return {"t": "call", "fn": str(xs[0]), "args": list(xs[1:])}

    def list_value(self, xs: list[Any]) -> Value:
        return {"t": "list", "items": list(xs)}

    def tuple_value(self, xs: list[Any]) -> Value:
        return {"t": "tuple", "items": list(xs)}

    def field(self, xs: list[Any]) -> tuple[str, Value]:
        return str(xs[0]), xs[1]

    def record(self, xs: list[Any]) -> Value:
        return {"t": "record", "fields": unique_fields(xs)}

    def object(self, xs: list[Any]) -> tuple[str, dict[str, Any]]:
        kind, name = map(str, xs[:2])
        fields: list[tuple[str, Value]] = []
        children: dict[str, Any] = {}
        for key, value in xs[2:]:
            if "kind" in value:
                if key in children:
                    raise ValueError(f"E_DUPLICATE_ID: {name}.{key}")
                children[key] = value
            else:
                fields.append((key, value))
        mapped = unique_fields(fields)
        if set(mapped) & set(children):
            raise ValueError(f"E_DUPLICATE_ID: field/child collision in {name}")
        return name, {"kind": kind, "fields": mapped, "children": children}

    def start(self, xs: list[Any]) -> dict[str, Any]:
        version = int(xs[0])
        if version != 1:
            raise ValueError("Unsupported language version")
        objects = {}
        for name, body in xs[1:]:
            if name in objects:
                raise ValueError(f"E_DUPLICATE_ID: {name}")
            objects[name] = body
        return {"version": version, "objects": objects}


def fraction(value: Value, unit: str | None = None) -> Fraction:
    if value["t"] not in ("number", "quantity"):
        raise ValueError("Expected a numeric value")
    result = Fraction(int(value["n"]), int(value["d"]))
    u = value.get("u")
    if u == "ms":
        result /= 1000
        u = "s"
    elif u == "kHz":
        result *= 1000
        u = "Hz"
    if u != unit:
        raise ValueError(f"E_UNIT: expected {unit!r}, received {u!r}")
    return result


def field_number(fields: dict[str, Value], name: str, default: str = "0", unit: str | None = None) -> Fraction:
    return fraction(fields[name], unit) if name in fields else Fraction(default)


def text(fields: dict[str, Value], name: str, default: str = "") -> str:
    return str(fields[name]["v"]) if name in fields else default


def reference(fields: dict[str, Value], name: str) -> str:
    value = fields[name]
    if value["t"] != "ref" or len(value["path"]) != 1:
        raise ValueError("This smoke-check path expects a top-level reference")
    return value["path"][0]


def expand_pattern(objects: dict[str, Any], name: str, ancestors: tuple[str, ...] = ()) -> tuple[Fraction, list[dict[str, Any]]]:
    """Selected pattern semantics: notes/hits/messages and finite uses.

    This intentionally does not typecheck all fields or execute expressions.
    """
    if name in ancestors:
        raise ValueError("E_PATTERN_CYCLE")
    pattern = objects[name]
    if pattern["kind"] != "pattern":
        raise ValueError("Expected pattern")
    length = fraction(pattern["fields"]["length"], "q")
    events: list[dict[str, Any]] = []
    for leaf_id, body in pattern["children"].items():
        fields = body["fields"]
        if body["kind"] in ("note", "hit", "message"):
            event = {"path": leaf_id, "kind": body["kind"], "at": fraction(fields["at"], "q"),
                     "onset_offset": field_number(fields, "onset_offset", unit="s"),
                     "order": field_number(fields, "order"), "source_fields": copy.deepcopy(fields),
                     "transpose": Fraction(0)}
            if body["kind"] == "note":
                event["dur"] = fraction(fields["dur"], "q")
                event["release_offset"] = field_number(fields, "release_offset", unit="s")
            events.append(event)
        elif body["kind"] == "use":
            child_length, inner = expand_pattern(objects, reference(fields, "pattern"), ancestors + (name,))
            at = fraction(fields["at"], "q")
            count = field_number(fields, "count", "1")
            scale = field_number(fields, "stretch", "1")
            shift = field_number(fields, "transpose", unit="ct")
            if count.denominator != 1 or count <= 0 or scale <= 0:
                raise ValueError("E_RANGE")
            if at + count * scale * child_length > length:
                raise ValueError("E_INTERVAL")
            for i in range(int(count)):
                for original in inner:
                    e = copy.deepcopy(original)
                    e["path"] = f"{leaf_id}/{i}/{e['path']}"
                    if "dur" in e and text(fields, "boundary", "spill") == "cut":
                        e["dur"] = min(e["dur"], child_length - e["at"])
                    e["at"] = at + scale * (i * child_length + e["at"])
                    if "dur" in e:
                        e["dur"] *= scale
                    e["transpose"] += shift
                    events.append(e)
        else:
            raise ValueError(f"Unsupported smoke-check pattern child: {body['kind']}")
    return length, events


def expand_places(doc: dict[str, Any]) -> list[dict[str, Any]]:
    objects = doc["objects"]
    result = []
    for name, body in objects.items():
        if body["kind"] != "place":
            continue
        f = body["fields"]
        length, source_events = expand_pattern(objects, reference(f, "pattern"))
        at = fraction(f["at"], "q")
        count = field_number(f, "count", "1")
        scale = field_number(f, "stretch", "1")
        transpose = field_number(f, "transpose", unit="ct")
        if count.denominator != 1 or count <= 0 or scale <= 0:
            raise ValueError("E_RANGE")
        local: dict[str, dict[str, Any]] = {}
        for i in range(int(count)):
            for source in source_events:
                e = copy.deepcopy(source)
                path = f"{i}/{e['path']}"
                if "dur" in e and text(f, "boundary", "spill") == "cut":
                    e["dur"] = min(e["dur"], length - e["at"])
                e["at"] = scale * (i * length + e["at"])
                if "dur" in e:
                    e["dur"] *= scale
                e["transpose"] += transpose
                e["path"] = f"{name}/{path}"
                local[path] = e
        for child in body["children"].values():
            if child["kind"] != "override":
                raise ValueError("Only overrides supported by this smoke checker")
            cf = child["fields"]
            path = cf["event"]["v"]
            if path not in local:
                raise ValueError("E_INSTANCE_TARGET")
            if "delete" in cf:
                if cf["delete"] != {"t": "boolean", "v": True}:
                    raise ValueError("E_RANGE")
                del local[path]
                continue
            for key, value in cf["set"]["fields"].items():
                if key in ("at", "dur"):
                    local[path][key] = fraction(value, "q")
                elif key in ("onset_offset", "release_offset"):
                    local[path][key] = fraction(value, "s")
                else:
                    raise ValueError(f"Override field outside smoke-check scope: {key}")
        for e in local.values():
            e["at"] += at
            result.append(e)
    return sorted(result, key=lambda e: (e["at"], e["path"]))


def noise(seed: int, node: str, frame: int, channel: int) -> float:
    b = (b"maac-noise-1\x00" + struct.pack("<Q", seed) + node.encode("ascii") + b"\x00"
         + struct.pack("<QI", frame, channel))
    r = int.from_bytes(hashlib.sha256(b).digest()[:8], "little") >> 11
    return 2 * (r / 2**53) - 1


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> None:
    parser = Lark((ROOT / "grammar.lark").read_text(), parser="lalr", maybe_placeholders=False)
    doc = ToSyntax().transform(parser.parse((ROOT / "example.maac").read_text()))
    schema = json.loads((ROOT / "syntax-tree.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    Draft202012Validator(schema).validate(doc)
    (ROOT / "example.syntax.json").write_text(json.dumps(doc, indent=2, ensure_ascii=False) + "\n")
    events = expand_places(doc)
    check(len(events) == 48, "Expected 48 example note occurrences")
    lookup = {e["path"]: e for e in events}
    check("upper_main/3/repeated/1/n2" in lookup, "Nested instance address")
    check(lookup["bass_main/7/n2"]["onset_offset"] == Fraction(1, 50), "Local override applied")
    check(lookup["bass_main/6/n2"]["onset_offset"] == 0, "Override must not affect sibling")
    check(lookup["bass_main/7/n2"]["at"] == Fraction(59, 2), "Placement score time")
    check(math.ceil((lookup["bass_main/7/n2"]["at"] / 2 + Fraction(1, 50)) * 48000) == 708960,
          "Worked example delayed frame")

    checks: list[dict[str, Any]] = []
    def exact(name: str, got: Fraction | int, expected: Fraction | int) -> None:
        check(got == expected, name)
        checks.append({"id": name, "actual": str(got), "expected": str(expected), "pass": True})
    def near(name: str, got: float, expected: float, tolerance: float = 1e-12) -> None:
        check(abs(got - expected) <= tolerance, name)
        checks.append({"id": name, "actual": got, "expected": expected, "absolute_tolerance": tolerance, "pass": True})

    exact("quarter_at_120_seconds", Fraction(60, 120), Fraction(1, 2))
    exact("triplet_sum_q", 3 * Fraction(1, 3), 1)
    exact("triplet_at_120_frames", Fraction(1, 3) * Fraction(1, 2) * 48000, 8000)
    exact("six_eight_bar_length", 6 * Fraction(4, 8), 3)
    exact("six_eight_bar_2_beat_4", 3 + 3 * Fraction(4, 8), Fraction(9, 2))
    exact("twenty_ms_seconds", fraction(rational("20", "quantity", "ms"), "s"), Fraction(1, 50))
    exact("one_point_five_khz", fraction(rational("1.5", "quantity", "kHz"), "Hz"), 1500)
    exact("onset_with_offset", (Fraction(4, 2) + Fraction(1, 50)) * 48000, 96960)
    exact("release_with_offset", (Fraction(5, 2) + Fraction(1, 50)) * 48000, 120960)
    exact("onset_only_shorter_gate", Fraction(5, 2) - (2 + Fraction(1, 50)), Fraction(12, 25))
    exact("step_tempo_six_q", 4 * Fraction(60, 120) + 2 * Fraction(60, 60), 4)
    exact("spill_gate_end", 3 + 2, 5)
    exact("cut_gate_end", min(3 + 2, 4), 4)
    exact("stretched_repetition_origin", 4 * Fraction(3, 2), 6)
    exact("fractional_event_ceil", math.ceil(Fraction(1, 3) * 100), 34)
    exact("subsample_gate_collapses", math.ceil(Fraction(2, 1000) * 100) - math.ceil(Fraction(1, 1000) * 100), 0)
    near("linear_tempo_ramp_seconds", 60 / 15 * math.log(180 / 120), 1.6218604324326575)
    near("exponential_hz_midpoint", 500 * (2000 / 500)**0.5, 1000)
    near("linear_db_midpoint", -12 + 12 * 0.5, -6)
    near("key_69_hz", 440 * 2**((69-69)/12), 440)
    near("key_60_hz", 440 * 2**((60-69)/12), 261.6255653005986)
    near("ratio_pitch", 440 * 1.5, 660)
    for frame in range(3):
        value = noise(1, "noise_a", frame, 0)
        check(-1 <= value < 1, "Noise range")
        checks.append({"id": f"noise_seed1_noise_a_frame{frame}_channel0", "value": value,
                       "note": "Reference value generated from the published hash formula, not an independent DSP test."})

    vectors = {
        "status": "Selected draft conformance arithmetic and reference values; not a full conformance suite",
        "checks": checks,
        "example": {"objects": len(doc["objects"]), "notes": len(events),
                    "delayed_event": "bass_main/7/n2", "delayed_onset_frame": 708960},
    }
    (ROOT / "conformance.json").write_text(json.dumps(vectors, indent=2) + "\n")
    report = {
        "surface_parse": "pass", "syntax_tree_schema": "pass",
        "example_expanded_note_count": len(events), "arithmetic_assertions": 22,
        "scope": "Syntax plus selected semantics only; no complete validator or renderer is included."
    }
    (ROOT / "check-results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
