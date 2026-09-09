#!/usr/bin/env python3
"""Nonmutating production/1 specification smoke checks; NOT a renderer.

Run with the pinned requirements-dev.txt environment. This deliberately checks
only the selected contracts enumerated in production-conformance.json. Importing
check_spec reuses its parser helpers without running its snapshot-writing main.
"""
from __future__ import annotations

import copy
import hashlib
import json
import math
from fractions import Fraction
from pathlib import Path

from check_spec import Draft202012Validator, Lark, ToSyntax, fraction, rational

ROOT = Path(__file__).resolve().parent
CAPABILITY = "maac.production/1"
NATIVE = {"fx.eq/1", "fx.compressor/1", "fx.reverb/1"}


class Invalid(ValueError):
    """Checker-local classification; not a normative MaaC diagnostic code."""


def require(condition, code):
    if not condition:
        raise Invalid(code)


def fields(value):
    require(value.get("t") == "record", "E_FIELD")
    return value["fields"]


def scalar(value):
    return value.get("v")


def enum(value):
    require(value.get("t") == "symbol", "E_FIELD")
    return value["v"]


def numeric(value, unit=None):
    try:
        result = fraction(value, unit)
        try:
            require(math.isfinite(float(result)), "E_NONFINITE")
        except OverflowError as exc:
            raise Invalid("E_NONFINITE") from exc
        return result
    except Invalid:
        raise
    except (ValueError, KeyError, ZeroDivisionError) as exc:
        raise Invalid("E_UNIT") from exc


def reference(value):
    require(value.get("t") == "ref" and len(value["path"]) == 1, "E_REFERENCE")
    return value["path"][0], value["port"]


def local_schema(schema):
    """Reject remote references before letting jsonschema resolve anything."""
    if isinstance(schema, dict):
        for key, value in schema.items():
            if key in ("$ref", "$dynamicRef"):
                require(isinstance(value, str) and value.startswith("#"), "E_SCHEMA_REFERENCE")
            local_schema(value)
    elif isinstance(schema, list):
        for value in schema:
            local_schema(value)


def parse(source):
    parser = Lark((ROOT / "grammar.lark").read_text(), parser="lalr", maybe_placeholders=False)
    return ToSyntax().transform(parser.parse(source))


def validate(doc, schema):
    """Selected production example semantics, with no import or DSP execution.

    Port inference covers the native processors and the example's core source,
    gain, pan and sum nodes. Other valid MaaC types require the full validator.
    """
    Draft202012Validator(json.loads((ROOT / "syntax-tree.schema.json").read_text())).validate(doc)
    objects = doc["objects"]
    projects = [o for o in objects.values() if o["kind"] == "project"]
    require(len(projects) == 1, "E_PROJECT")
    project = projects[0]["fields"]
    require(CAPABILITY in [scalar(x) for x in project.get("requires", {}).get("items", [])], "E_CAPABILITY")
    require(numeric(project["rate"], "Hz") == 48000, "E_RATE")
    extensions = [o for o in objects.values() if o["kind"] == "extension" and scalar(o["fields"].get("namespace", {})) == CAPABILITY]
    require(all(o in extensions for o in objects.values() if o["kind"] == "extension"), "E_CAPABILITY")
    require(len(extensions) <= 1, "E_EXTENSION")
    ef = extensions[0]["fields"] if extensions else None
    if ef is not None:
        validate_extension(ef, objects, schema)

    nodes = {name: o for name, o in objects.items() if o["kind"] == "node"}
    channels = {}
    kinds = {}
    parameter_contracts = {}
    for name, node in nodes.items():
        nf = node["fields"]
        kind = scalar(nf.get("type", {}))
        require(kind in NATIVE | {"core.sine/1", "core.gain/1", "core.pan/1", "core.sum/1"}, "E_CAPABILITY")
        kinds[name] = kind
        config = fields(nf.get("config", {"t": "record", "fields": {}}))
        if kind in NATIVE:
            require("channels" in config, "E_LAYOUT")
        count = numeric(config.get("channels", rational("1")))
        if kind == "core.pan/1":
            count = 2
        require(count in (1, 2), "E_LAYOUT")
        channels[name] = int(count)
        if kind not in NATIVE:
            continue
        require(not node["children"] and set(nf) <= {"type", "config", "params", "label"}, "E_FIELD")
        params = fields(nf.get("params", {"t": "record", "fields": {}}))
        if kind == "fx.eq/1":
            require(set(config) <= {"channels", "mode"}, "E_FIELD")
            mode = enum(config.get("mode", {}))
            require(mode in {"peak", "low_shelf", "high_shelf", "low_pass", "high_pass"}, "E_MODE")
            allowed = {"frequency", "q", "gain"} if mode == "peak" else ({"frequency", "gain"} if "shelf" in mode else {"frequency", "q"})
            bounds = {"frequency": ("Hz", 0, 24000), "q": (None, Fraction(1, 10), 18), "gain": ("dB", -24, 24)}
        elif kind == "fx.compressor/1":
            require(set(config) <= {"channels", "detector", "sidechain_channels"}, "E_FIELD")
            detector = enum(config.get("detector", {"t": "symbol", "v": "internal"}))
            require(detector in {"internal", "external"}, "E_DETECTOR")
            if detector == "external":
                require("sidechain_channels" in config and numeric(config["sidechain_channels"]) in (1, 2), "E_SIDECHAIN")
            else:
                require("sidechain_channels" not in config, "E_SIDECHAIN")
            bounds = {"threshold": ("dB", -120, 24), "ratio": (None, 1, 100), "knee": ("dB", 0, 24), "attack": ("s", 0, 10), "release": ("s", 0, 30), "makeup": ("dB", -24, 24)}
            allowed = set(bounds)
        else:
            require(set(config) <= {"channels", "predelay", "damping"}, "E_FIELD")
            require(0 <= numeric(config.get("predelay", rational("0", "quantity", "s")), "s") <= Fraction(1, 4), "E_RANGE")
            require(0 <= numeric(config.get("damping", rational("1/2"))) <= 1, "E_RANGE")
            bounds = {"decay": ("s", Fraction(1, 10), 30), "mix": (None, 0, 1)}
            allowed = set(bounds)
        parameter_contracts[name] = {key: bounds[key] for key in allowed}
        require(set(params) <= allowed, "E_PARAMETER")
        for pname, value in params.items():
            unit, lo, hi = bounds[pname]
            number = numeric(value, unit)
            require(lo < number < hi if pname == "frequency" else lo <= number <= hi, "E_RANGE")

    def port(value, output=True):
        name, pname = reference(value)
        require(name in nodes, "E_REFERENCE")
        require(pname == "out" if output else pname in ("in", "sidechain"), "E_PORT")
        if not output and pname == "sidechain":
            cf = fields(nodes[name]["fields"].get("config", {"t": "record", "fields": {}}))
            require(kinds[name] == "fx.compressor/1" and scalar(cf.get("detector", {"v": "internal"})) == "external", "E_SIDECHAIN")
            return int(numeric(cf["sidechain_channels"]))
        return 1 if not output and kinds[name] == "core.pan/1" else channels[name]

    sidechains = set()
    main_inputs = {}
    for obj in objects.values():
        if obj["kind"] == "connect":
            cf = obj["fields"]
            require(port(cf["from"]) == port(cf["to"], False), "E_LAYOUT")
            dest, p = reference(cf["to"])
            if p == "sidechain":
                require(dest not in sidechains, "E_SIDECHAIN")
                sidechains.add(dest)
            else:
                main_inputs[dest] = main_inputs.get(dest, 0) + 1
        elif obj["kind"] == "automation":
            target = obj["fields"]["target"]
            path = target.get("path", [])
            if path and kinds.get(path[0]) in NATIVE:
                require(len(path) == 3 and path[1] == "params" and target.get("port") is None, "E_AUTOMATION")
                kind = kinds[path[0]]
                allowed = {"fx.eq/1": {"frequency", "q", "gain"}, "fx.compressor/1": {"threshold", "ratio", "knee", "attack", "release", "makeup"}, "fx.reverb/1": {"decay", "mix"}}[kind]
                require(path[2] in allowed, "E_AUTOMATION")
                if kind == "fx.eq/1":
                    mode = scalar(fields(nodes[path[0]]["fields"]["config"])["mode"])
                    require(not (path[2] == "gain" and mode.endswith("pass")) and not (path[2] == "q" and mode.endswith("shelf")), "E_AUTOMATION")
                curve_id, curve_port = reference(obj["fields"]["curve"])
                require(curve_id in objects and curve_port is None and objects[curve_id]["kind"] == "curve", "E_REFERENCE")
                curve = objects[curve_id]["fields"]
                require("clock" in curve, "E_AUTOMATION")
                clock = enum(curve["clock"])
                require(clock in ("score", "seconds"), "E_AUTOMATION")
                unit, lo, hi = parameter_contracts[path[0]][path[2]]
                points = curve["points"]["items"]
                require(len(points) > 0, "E_AUTOMATION")
                previous_time = None
                for index, point in enumerate(points):
                    require(point["t"] == "tuple" and len(point["items"]) == 3, "E_AUTOMATION")
                    time, value, interpolation = point["items"]
                    time = numeric(time, "q" if clock == "score" else "s")
                    require(previous_time is None or time > previous_time, "E_AUTOMATION")
                    require(index != 0 or time == 0, "E_AUTOMATION")
                    previous_time = time
                    value = numeric(value, unit)
                    require(lo < value < hi if path[2] == "frequency" else lo <= value <= hi, "E_RANGE")
                    mode = enum(interpolation)
                    require(mode in ("step", "linear", "exponential"), "E_AUTOMATION")
                    require(index != len(points) - 1 or mode == "step", "E_AUTOMATION")
                    if mode == "exponential":
                        require(unit != "dB" and value > 0, "E_AUTOMATION")
                        if index + 1 < len(points):
                            require(numeric(points[index + 1]["items"][1], unit) > 0, "E_AUTOMATION")
    for name, node in nodes.items():
        if kinds[name] in NATIVE:
            require(main_inputs.get(name, 0) == 1, "E_INPUT")
        if kinds[name] == "fx.compressor/1":
            cf = fields(node["fields"].get("config", {"t": "record", "fields": {}}))
            require((scalar(cf.get("detector", {"v": "internal"})) == "external") == (name in sidechains), "E_SIDECHAIN")
    port(project["output"])
    if ef is None:
        return
    deliveries = fields(fields(ef["data"])["deliveries"])
    for delivery in deliveries.values():
        df = fields(delivery)
        require(numeric(df["rate"], "Hz") in (44100, 48000, 96000), "E_RATE")
        masters = []
        for target in fields(df["targets"]).values():
            tf = fields(target)
            port(tf["output"])
            if scalar(tf["role"]) == "master":
                masters.append(tf)
            dither = fields(tf["dither"])
            if scalar(dither["type"]) == "tpdf":
                require(0 <= numeric(dither["seed"]) < 2**64, "E_SEED")
            for limit in fields(tf.get("limits", {"t": "record", "fields": {}})).values():
                lf = fields(limit)
                for bound in ("min", "max"):
                    if bound in lf:
                        numeric(lf[bound])
                if "min" in lf and "max" in lf:
                    require(numeric(lf["min"]) <= numeric(lf["max"]), "E_LIMIT")
        require(len(masters) == 1, "E_MASTER")
        require(masters[0]["output"] == project["output"], "E_MASTER")


def validate_extension(ef, objects, schema):
    require(set(ef) == {"namespace", "schema", "render_affecting", "data"}, "E_FIELD")
    require(ef["render_affecting"] == {"t": "boolean", "v": True}, "E_EXTENSION")
    asset_id, asset_port = reference(ef["schema"])
    require(asset_id in objects and asset_port is None and objects[asset_id]["kind"] == "asset", "E_REFERENCE")
    af = objects[asset_id]["fields"]
    require(scalar(af.get("kind", {})) == "descriptor", "E_SCHEMA_ASSET")
    require(scalar(af.get("path", {})) == "production.schema.json", "E_SCHEMA_ASSET")
    require(scalar(af.get("hash", {})) == "sha256:" + hashlib.sha256((ROOT / "production.schema.json").read_bytes()).hexdigest(), "E_HASH")
    local_schema(schema)
    try:
        Draft202012Validator(schema).validate(ef["data"])
    except Exception as exc:
        # Preserve one bounded schema diagnostic instead of exposing resolver internals.
        from jsonschema.exceptions import ValidationError
        if not isinstance(exc, ValidationError):
            raise
        raise Invalid("E_SCHEMA") from exc


def eq_impulse(mode, frequency, q, gain, count):
    """Small independent cookbook reference for fixture arithmetic only."""
    w = 2 * math.pi * frequency / 48000
    c, a = math.cos(w), math.sin(w) / (2 * q)
    A = 10 ** (gain / 40)
    if mode == "low_pass":
        b, den = [(1-c)/2, 1-c, (1-c)/2], [1+a, -2*c, 1-a]
    elif mode == "high_pass":
        b, den = [(1+c)/2, -(1+c), (1+c)/2], [1+a, -2*c, 1-a]
    else:
        b, den = [1+a*A, -2*c, 1-a*A], [1+a/A, -2*c, 1-a/A]
    b = [x / den[0] for x in b]
    den = [x / den[0] for x in den]
    x1 = x2 = y1 = y2 = 0.0
    out = []
    for frame in range(count):
        x = 1.0 if frame == 0 else 0.0
        y = b[0]*x + b[1]*x1 + b[2]*x2 - den[1]*y1 - den[2]*y2
        out.append(y)
        x2, x1, y2, y1 = x1, x, y1, y
    return out


def reduction(level, threshold, ratio, knee):
    delta = level - threshold
    if knee == 0:
        return max(0, (1 - 1 / ratio) * delta)
    if delta <= -knee / 2:
        return 0.0
    if delta >= knee / 2:
        return (1 - 1 / ratio) * delta
    return (1 - 1 / ratio) * (delta + knee / 2)**2 / (2 * knee)


def smooth(previous, target, attack, release):
    time = attack if target > previous else release
    alpha = 0.0 if time == 0 else math.exp(-1 / (48000 * time))
    return alpha * previous + (1 - alpha) * target


class FirstArrivalNetwork:
    """Bounded mono arithmetic probe, not an audio transport or renderer.

    Implements enough declared state to independently check arrival, predelay,
    reset, and the read-before-write rule. No SRC, encoding or metering support.
    """
    lengths = (1493, 1601, 1747, 1867, 1999, 2137, 2281, 2437)

    def __init__(self, predelay=0):
        self.predelay = predelay
        self.reset()

    def reset(self):
        self.frame = 0
        self.pre = [0.0] * self.predelay
        self.allpasses = [[0.0] * n for n in (149, 211)]
        self.rings = [[0.0] * n for n in self.lengths]
        self.previous = [0.0] * 8

    def step(self, x):
        n = self.frame
        if self.predelay:
            idx = n % self.predelay
            self.pre[idx], x = x, self.pre[idx]
        for ring in self.allpasses:
            idx = n % len(ring)
            y = ring[idx] - 0.5*x
            ring[idx] = x + 0.5*y
            x = y
        old = [ring[n % len(ring)] for ring in self.rings]
        damped = [0.75*r + 0.25*p for r, p in zip(old, self.previous)]
        # H_8 normalized Sylvester signs: (-1)^popcount(row & column).
        mixed = [sum((-1 if (i & j).bit_count() % 2 else 1) * damped[j] for j in range(8)) / math.sqrt(8) for i in range(8)]
        for i, ring in enumerate(self.rings):
            ring[n % len(ring)] = x / math.sqrt(8) + 10**(-3*len(ring)/(48000*1.5))*mixed[i]
        self.previous = old
        self.frame += 1
        return sum(old) / math.sqrt(8)


def dither_counts(seed, delivery, target, frame, channel):
    values = []
    for j in range(2):
        message = f"maac.dither.sha256-tpdf/1\n{seed}\n{delivery}\n{target}\n{frame}\n{channel}\n{j}\n".encode("utf-8")
        values.append((int.from_bytes(hashlib.sha256(message).digest()[:8], "big") >> 11) / 2**53)
    return values[0] - values[1]


def quantize(x, bits, dither=0.0):
    require(math.isfinite(x) and math.isfinite(dither), "E_NONFINITE")
    maximum = 2**(bits-1) - 1
    require(-1 <= x <= 1, "E_OVERLOAD")
    scaled = x * maximum + dither
    require(math.isfinite(scaled), "E_NONFINITE")
    require(-maximum <= scaled <= maximum, "E_OVERLOAD")
    magnitude = abs(scaled)
    whole = math.floor(magnitude)
    rounded = whole + (magnitude - whole >= 0.5)
    return rounded if scaled >= 0 else -rounded


def near(actual, expected, tolerance=1e-12):
    assert math.isfinite(actual) and abs(actual - expected) <= tolerance, (actual, expected)


def true_peak_4x(samples):
    """Exact Annex 2 convolution for bounded fixtures, independent of Rust DSP."""
    coefficients = [list(map(Fraction, row.split())) for row in """
 0.001708984375  -0.0291748046875 -0.0189208984375 -0.00830078125
 0.010986328125   0.029296875      0.0330810546875  0.014892578125
-0.0196533203125 -0.0517578125    -0.0582275390625 -0.026611328125
 0.033203125     0.089111328125   0.1015625        0.047607421875
-0.0594482421875 -0.16650390625   -0.2003173828125 -0.102294921875
 0.1373291015625  0.465087890625   0.77978515625    0.97216796875
 0.97216796875    0.77978515625    0.465087890625   0.1373291015625
-0.102294921875  -0.2003173828125 -0.16650390625   -0.0594482421875
 0.047607421875   0.1015625        0.089111328125   0.033203125
-0.026611328125  -0.0582275390625 -0.0517578125    -0.0196533203125
 0.014892578125   0.0330810546875  0.029296875      0.010986328125
-0.00830078125   -0.0189208984375 -0.0291748046875  0.001708984375
""".strip().splitlines()]
    output = [sum((coefficients[k][phase] * samples[n-k]
                   for k in range(12) if 0 <= n-k < len(samples)), Fraction(0))
              for n in range(len(samples) + 11) for phase in range(4)]
    sample_peak = max(map(abs, samples), default=Fraction(0))
    interpolated_peak = max(map(abs, output), default=Fraction(0))
    return output, interpolated_peak, max(sample_peak, interpolated_peak)


def arithmetic(fixtures):
    checks = 0
    assert fixtures["analysis_identity"] == "maac.analysis.bs1770-5/2"
    assert fixtures["true_peak_profile"] == "maac.truepeak.bs1770-5.annex2-4x/1"
    for case in fixtures["true_peak"]:
        output, interpolated, amplitude = true_peak_4x(list(map(Fraction, case["input"])))
        assert len(output) == case["expected_output_frames"]
        assert interpolated == Fraction(case["expected_interpolated_peak"])
        assert amplitude == Fraction(case["expected_amplitude"])
        assert output[-1] == Fraction(case["expected_last_sample"])
        checks += 1
    for case in fixtures["eq"]:
        actual = eq_impulse(case["mode"], case["frequency_hz"], case["q"], case["gain_db"], len(case["expected"]))
        for got, expected in zip(actual, case["expected"]):
            near(got, expected)
        checks += 1
    for case in fixtures["compressor_knee"]:
        near(reduction(case["level_db"], -18, case["ratio"], case["knee_db"]), case["expected_reduction_db"])
        checks += 1
    for case in fixtures["compressor_time"]:
        value = case["initial_db"]
        for _ in range(case["frames"]):
            value = smooth(value, case["target_db"], case["attack_seconds"], case["release_seconds"])
        near(value, case["expected_db"], 2e-11)
        checks += 1
    # Linked detector is max absolute amplitude; one gain preserves L/R ratio.
    peak = max(abs(x) for x in (-0.5, 0.25))
    near(20 * math.log10(peak), -6.020599913279624)
    gain = 10**(-reduction(20*math.log10(peak), -18, 4, 0)/20)
    near((-0.5 * gain) / (0.25 * gain), -2)
    checks += 1
    for case in fixtures["reverb"]:
        network = FirstArrivalNetwork(case["predelay_frames"])
        def probe():
            result = []
            for n in range(case["first_wet_frame"] + 1):
                result.append(network.step(1.0 if n == 0 else 0.0))
            return result
        result = probe()
        assert all(x == 0 for x in result[:-1])
        near(result[-1], case["expected_first_wet"])
        network.reset()
        assert probe() == result
        checks += 1
    for case in fixtures["quantization"]:
        value = float(Fraction(case["input"]))
        dither = float(Fraction(case.get("dither_counts", "0")))
        if "error" in case:
            try:
                quantize(value, case["bits"], dither)
            except Invalid as exc:
                assert str(exc) == case["error"]
            else:
                raise AssertionError(case)
        else:
            assert quantize(value, case["bits"], dither) == case["expected_integer"]
            if "expected_decoded" in case:
                assert Fraction(case["expected_integer"], 2**(case["bits"]-1)) == Fraction(case["expected_decoded"])
        checks += 1
    for invalid in (math.nan, math.inf, -math.inf):
        try:
            quantize(invalid, 16)
        except Invalid as exc:
            assert str(exc) == "E_NONFINITE"
        else:
            raise AssertionError("Nonfinite conversion accepted")
        checks += 1
    for case in fixtures["dither"]:
        actual = dither_counts(case["seed"], case["delivery"], case["target"], case["frame"], case["channel"])
        assert actual == float.fromhex(case["expected_hex"])
        checks += 1
    for case in fixtures["frame_counts"]:
        seconds = Fraction(case["duration_seconds"])
        actual = math.ceil(seconds * case["rate"])
        assert actual == case["expected_frames"]
        if "wrong_engine_rounded_frames" in case:
            wrong = math.ceil(Fraction(math.ceil(seconds*48000), 48000) * case["rate"])
            assert wrong == case["wrong_engine_rounded_frames"] and actual != wrong
        checks += 1
    return checks


def main():
    source = (ROOT / "examples/production.maac").read_text()
    schema = json.loads((ROOT / "production.schema.json").read_text())
    fixtures = json.loads((ROOT / "production-conformance.json").read_text())
    local_schema(schema)
    Draft202012Validator.check_schema(schema)
    doc = parse(source)
    validate(doc, schema)
    # Effects-only is valid. Production does not force delivery declarations.
    effects_only = copy.deepcopy(doc)
    del effects_only["objects"]["production_deliveries"]
    validate(effects_only, schema)
    validate(parse(source.replace("rate = 44100Hz;", "rate = 48000Hz;")), schema)
    valid_count = 3
    for mode in ("low_pass", "high_pass", "low_shelf", "high_shelf"):
        variant = copy.deepcopy(doc)
        eq = variant["objects"]["bass_eq"]["fields"]
        eq["config"]["fields"]["mode"]["v"] = mode
        del eq["params"]["fields"]["gain" if mode.endswith("pass") else "q"]
        validate(variant, schema)
        valid_count += 1
    defaults = copy.deepcopy(doc)
    for node in defaults["objects"].values():
        if node["kind"] == "node" and scalar(node["fields"].get("type", {})) in NATIVE:
            node["fields"].pop("params", None)
    cf = defaults["objects"]["ducked_bass"]["fields"]["config"]["fields"]
    del cf["detector"], cf["sidechain_channels"]
    del defaults["objects"]["pulse_detector"]
    rf = defaults["objects"]["bass_room"]["fields"]["config"]["fields"]
    del rf["predelay"], rf["damping"]
    validate(defaults, schema)
    valid_count += 1
    for case in fixtures["invalid_source_mutations"]:
        assert case["replace"] in source, case["id"]
        changed = source.replace(case["replace"], case["with"], 1)
        try:
            validate(parse(changed), schema)
        except Invalid as exc:
            assert str(exc) == case["error"], (case["id"], str(exc), case["error"])
        else:
            raise AssertionError("Accepted invalid fixture: " + case["id"])
    remote_schema = copy.deepcopy(schema)
    remote_schema["$ref"] = "https://example.invalid/schema.json"
    try:
        local_schema(remote_schema)
    except Invalid as exc:
        assert str(exc) == "E_SCHEMA_REFERENCE"
    else:
        raise AssertionError("Accepted remote schema reference")
    counts = arithmetic(fixtures)
    print(json.dumps({"status": "pass", "scope": "production specification smoke checks only; Rust renderer verification is separate", "valid_documents": valid_count, "invalid_documents": len(fixtures["invalid_source_mutations"]), "schema_reference_checks": 1, "arithmetic_cases": counts, "tracked_artifact_writes": 0}, indent=2))


if __name__ == "__main__":
    main()
