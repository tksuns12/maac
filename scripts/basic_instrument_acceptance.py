#!/usr/bin/env python3
"""Verify freshly installed built-in instruments outside the source checkout.

First run instrument_acceptance.py, including its offline cargo install, legacy
acceptance, reusable-instrument checks, and disposable specification smoke test.
Then retain fresh catalog JSON, 25 float32/PCM16 auditions, and standalone plans
under target/basic-acceptance. This runner uses only Python's standard library;
--python selects the existing interpreter with Lark/jsonschema for spec smoke.
Numerical audio checks do not establish human listening or perceptual quality.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from typing import Any
import uuid

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "target" / "basic-acceptance"
INSTALLED = ROOT / "target" / "install" / "bin" / "maac"
BASIC_SOURCE = ROOT / "stdlib" / "basic" / "1.0.0.maac"
BASIC_ID = "std/basic/1.0.0"
BASIC_PATH = "@builtin/std/basic/1.0.0.maac"
EXPECTED_NAMES = frozenset("""
mellow_piano bright_piano electric_piano organ nylon_guitar steel_guitar muted_guitar
finger_bass pick_bass sub_bass synth_bass strings warm_pad flute bell synth_lead
kick snare closed_hat open_hat low_tom high_tom clap crash
""".split())
NOISE_INSTRUMENTS = frozenset({"snare", "closed_hat", "open_hat", "clap", "crash"})

# Import only pure helpers; prevent runner preparation from creating script caches.
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / "scripts"))
from acceptance import parse_json, sha256, wav_info  # noqa: E402


class GateFailure(RuntimeError):
    """A required observable acceptance condition failed."""


def source_manifest() -> dict[str, str]:
    files = {
        ROOT / name for name in (
            "Cargo.toml", "Cargo.lock", "check_spec.py", "grammar.lark",
            "syntax-tree.schema.json", "example.maac", "evening-window.maac",
            "check-results.json", "conformance.json", "example.syntax.json",
            "docs/tutorial.md",
        )
    }
    for name in ("src", "stdlib", "examples", "scripts", "tests"):
        files.update(p for p in (ROOT / name).rglob("*")
                     if p.is_file() and "__pycache__" not in p.parts)
    return {str(p.relative_to(ROOT)): sha256(p) for p in sorted(files)}


class Evidence:
    def __init__(self) -> None:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        self.run_id = f"{stamp}-{uuid.uuid4().hex[:8]}"
        self.run = ARTIFACTS / "runs" / self.run_id
        self.logs = self.run / "command-logs"
        self.auditions = ARTIFACTS / "auditions" / self.run_id
        self.logs.mkdir(parents=True, exist_ok=False)
        self.auditions.mkdir(parents=True, exist_ok=False)
        self.commands: list[dict[str, Any]] = []
        self.checks: list[dict[str, Any]] = []
        self.binary_hash: str | None = None

    def require(self, name: str, passed: bool, detail: Any = None) -> None:
        self.checks.append({"name": name, "ok": bool(passed), "detail": detail})
        if not passed:
            raise GateFailure(name)

    def command(self, name: str, args: list[str | Path], *, cwd: Path) -> subprocess.CompletedProcess[str]:
        argv = [str(arg) for arg in args]
        uses_installed = argv[0] == str(INSTALLED)
        if uses_installed:
            self.require(f"{name}.stable_binary_before", sha256(INSTALLED) == self.binary_hash)
        try:
            completed = subprocess.run(argv, cwd=cwd, text=True, capture_output=True,
                                       check=False, errors="replace")
        except OSError as error:
            completed = subprocess.CompletedProcess(argv, 127, "", str(error))
        safe_name = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        log = self.logs / f"{len(self.commands):03d}-{safe_name}"
        Path(f"{log}.stdout").write_text(completed.stdout, encoding="utf-8")
        Path(f"{log}.stderr").write_text(completed.stderr, encoding="utf-8")
        self.commands.append({"name": name, "argv": argv, "command": shlex.join(argv),
                              "cwd": str(cwd), "returncode": completed.returncode,
                              "stdout_log": str(Path(f"{log}.stdout").relative_to(ROOT)),
                              "stderr_log": str(Path(f"{log}.stderr").relative_to(ROOT))})
        if uses_installed:
            self.require(f"{name}.stable_binary_after", sha256(INSTALLED) == self.binary_hash)
        return completed

    def cli(self, name: str, command: str, *args: str | Path, cwd: Path) -> dict[str, Any]:
        process = self.command(name, [INSTALLED, "--json", command, *args], cwd=cwd)
        parsed = parse_json(process.stdout)
        self.require(name, process.returncode == 0 and parsed is not None
                     and parsed.get("ok") is True and parsed.get("command") == command,
                     parsed or {"returncode": process.returncode})
        assert parsed is not None
        return parsed


def inspect_audio(path: Path, encoding: str, frames: int) -> dict[str, Any]:
    info = wav_info(path)
    info["normalized_peak"] = info["peak"] / (32768 if encoding == "pcm16" else 1)
    if not (info["encoding"] == encoding and info["channels"] == 2
            and info["sample_rate_hz"] == 48000 and info["frames"] == frames
            and info["finite"] and info["nonzero_samples"] > 0
            and 0 < info["normalized_peak"] < 1):
        raise GateFailure(f"invalid, silent, or clipped {encoding} audio: {path}")
    info["sha256"] = sha256(path)
    return info


def inspect_plan(plan: dict[str, Any], source_name: str, source_hash: str,
                 library_hash: str, expected_instruments: set[str]) -> dict[str, Any]:
    resources = plan.get("instruments")
    if plan.get("version") != 2 or not isinstance(resources, dict):
        raise GateFailure("compiled plan lacks version 2 instrument resources")
    identities = {item["path"]: item["hash"] for item in resources["source_files"]}
    if (resources["entry_source"] != source_name
            or identities != {source_name: source_hash, BASIC_PATH: library_hash}):
        raise GateFailure("compiled source identities do not match copied source and built-in library")
    dependencies = resources["dependencies"]
    if not (len(dependencies) == 1 and dependencies[0]["source"] == source_name
            and dependencies[0]["alias"] == "basic"
            and dependencies[0]["path"] == BASIC_PATH
            and dependencies[0]["hash"] == library_hash):
        raise GateFailure("compiled built-in dependency identity differs")
    programs = resources["programs"]
    by_name = {p["source"]["object"]: p for p in programs}
    if not expected_instruments <= by_name.keys():
        raise GateFailure("compiled plan lacks an expected instrument graph")
    if resources["wavetables"] or resources["wavetable_sources"]:
        raise GateFailure("basic instruments unexpectedly embed sample assets")
    seeds = []
    for program in programs:
        if program["source"]["file"] != BASIC_PATH or not program["voice"]["nodes"]:
            raise GateFailure("compiled graph source or nodes missing")
        has_noise = False
        for stage in (program["voice"], program.get("shared")):
            if stage is None:
                continue
            for node in stage["nodes"]:
                processor = node["processor"]
                if processor.get("kind") == "synth.noise/1":
                    seed = processor.get("seed")
                    if type(seed) is not int or not 1 <= seed <= 0xFFFFFFFF:
                        raise GateFailure("noise seed is absent or outside the nonzero u32 contract")
                    has_noise = True
                    seeds.append({"instrument": program["source"]["object"],
                                  "node": node["id"], "seed": seed})
        if program["source"]["object"] in NOISE_INSTRUMENTS and not has_noise:
            raise GateFailure("noise-based percussion graph has no embedded seeded noise")
    return {"version": 2, "entry_source": source_name, "source_files": resources["source_files"],
            "dependencies": dependencies, "programs": sorted(by_name), "noise_seeds": seeds,
            "sample_assets": 0}


def verify_prerequisite(evidence: Evidence, python: str, result: dict[str, Any]) -> None:
    completed = evidence.command("fresh_install_and_instrument_acceptance",
                                 [sys.executable, ROOT / "scripts/instrument_acceptance.py",
                                  "--python", python], cwd=ROOT)
    summary = parse_json(completed.stdout)
    prerequisite_path = ROOT / "target/instrument-acceptance/results.json"
    prerequisite = json.loads(prerequisite_path.read_text(encoding="utf-8"))
    legacy_path = ROOT / "target/acceptance/results.json"
    legacy = json.loads(legacy_path.read_text(encoding="utf-8"))
    evidence.require("fresh_install_and_instrument_acceptance", completed.returncode == 0
                     and summary is not None and summary.get("ok") is True
                     and prerequisite.get("ok") is True and legacy.get("ok") is True
                     and any(c.get("name") == "offline_install" and c.get("ok") is True
                             for c in legacy.get("checks", [])))
    evidence.require("installed_executable_present", INSTALLED.is_file() and os.access(INSTALLED, os.X_OK))
    for name, value in (("instrument-acceptance", prerequisite), ("legacy-acceptance", legacy)):
        (evidence.run / f"{name}.results.json").write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    evidence.binary_hash = sha256(INSTALLED)
    result["installed_binary"] = str(INSTALLED.relative_to(ROOT))
    result["installed_binary_sha256_before"] = evidence.binary_hash
    result["prerequisite"] = {"ok": True, "fresh_offline_install": True,
                              "legacy_acceptance": True, "instrument_acceptance": True,
                              "disposable_spec_smoke": True}


def verify_catalog(evidence: Evidence, outside: Path, library_hash: str) -> dict[str, Any]:
    response = evidence.cli("catalog", "instruments", cwd=outside)
    catalog = response["catalog"]
    instruments = catalog["instruments"]
    evidence.require("catalog_identity_and_names", catalog["library"] == BASIC_ID
                     and catalog["source_path"] == BASIC_PATH and catalog["source_hash"] == library_hash
                     and len(instruments) == 24 and {i["name"] for i in instruments} == EXPECTED_NAMES)
    (evidence.run / "catalog.json").write_text(json.dumps(response, indent=2) + "\n", encoding="utf-8")
    for item in instruments:
        name = item["name"]
        evidence.require(f"catalog_controls.{name}", item["channels"] == 2
                         and set(item["controls"]) == {"level", "brightness", "release", "pan"}
                         and bool(item["description"]) and bool(item["guidance"]["tested_pitches"]))
        detail = evidence.cli(f"detail.{name}", "instruments", name, cwd=outside)
        evidence.require(f"detail_matches_catalog.{name}", detail["instrument"] == item)
        (evidence.run / f"{name}.detail.json").write_text(json.dumps(detail, indent=2) + "\n", encoding="utf-8")
        usage = outside / f"usage_{name}.maac"
        usage.write_text(item["usage"], encoding="utf-8")
        evidence.cli(f"usage.{name}", "check", usage.name, cwd=outside)
        usage.unlink()
    return catalog


def verify_failures(evidence: Evidence, outside: Path) -> None:
    unknown = evidence.command("unknown_catalog_instrument", [INSTALLED, "--json", "instruments",
                                                              "does_not_exist"], cwd=outside)
    error = parse_json(unknown.stdout)
    evidence.require("unknown_catalog_instrument_json", unknown.returncode != 0 and error is not None
                     and error.get("ok") is False and isinstance(error.get("code"), str), error)
    base = (ROOT / "examples/basic/mellow_piano.maac").read_text(encoding="utf-8")
    failures = {
        "unknown_instrument": base.replace("&basic.mellow_piano", "&basic.does_not_exist"),
        "unknown_version": base.replace(BASIC_ID, "std/basic/9.9.9"),
        "mixed_import": base.replace(f'builtin = "{BASIC_ID}";',
                                     f'builtin = "{BASIC_ID}"; path = "absent.maac"; hash = "sha256:{"0" * 64}";'),
    }
    for name, text in failures.items():
        evidence.require(f"failure_fixture_changed.{name}", text != base)
        source = outside / f"invalid_{name}.maac"
        source.write_text(text, encoding="utf-8")
        for command, extension in (("compile", "json"), ("build", "wav")):
            output = evidence.run / f"must_not_exist_{name}.{extension}"
            process = evidence.command(f"reject.{name}.{command}",
                                       [INSTALLED, "--json", command, source.name, "-o", output], cwd=outside)
            error = parse_json(process.stdout)
            evidence.require(f"reject_without_artifact.{name}.{command}", process.returncode != 0
                             and error is not None and error.get("ok") is False
                             and isinstance(error.get("code"), str) and not output.exists(), error)
        source.unlink()


def verify_example(evidence: Evidence, outside: Path, name: str, library_hash: str) -> dict[str, Any]:
    original = ROOT / "examples" / "basic" / f"{name}.maac"
    copied = outside / original.name
    shutil.copyfile(original, copied)
    source_hash = f"sha256:{sha256(copied)}"
    expected = set(re.findall(r"instrument\s*=\s*&basic\.([A-Za-z0-9_]+)", copied.read_text(encoding="utf-8")))
    evidence.require(f"example_instrument_selection.{name}", bool(expected) and expected <= EXPECTED_NAMES
                     and (name == "full_band" or expected == {name}))
    wav = evidence.auditions / f"{name}.float32.wav"
    repeat = evidence.run / f"{name}.repeat.float32.wav"
    pcm = evidence.auditions / f"{name}.pcm16.wav"
    plan_path = evidence.run / f"{name}.v2.performance.json"
    retained = evidence.run / f"{name}.retained.float32.wav"
    build = evidence.cli(f"build.{name}", "build", copied.name, "-o", wav, cwd=outside)
    frames = build["frames"]
    evidence.cli(f"repeat.{name}", "build", copied.name, "-o", repeat, cwd=outside)
    evidence.cli(f"pcm16.{name}", "build", copied.name, "-o", pcm, "--format", "pcm16", cwd=outside)
    compiled = evidence.cli(f"compile.{name}", "compile", copied.name, "-o", plan_path, cwd=outside)
    evidence.require(f"compiled_frames.{name}", compiled["frames"] == frames)
    float_info = inspect_audio(wav, "float32", frames)
    pcm_info = inspect_audio(pcm, "pcm16", frames)
    evidence.require(f"audio_and_repeat.{name}", sha256(repeat) == float_info["sha256"],
                     {"float32": float_info, "pcm16": pcm_info, "repeat_sha256": sha256(repeat)})
    plan_info = inspect_plan(json.loads(plan_path.read_text(encoding="utf-8")), copied.name,
                             source_hash, library_hash, expected)
    evidence.require(f"plan_resources.{name}", True, plan_info)
    copied.unlink()
    evidence.require(f"copied_composition_removed.{name}", not copied.exists())
    evidence.cli(f"retained.{name}", "render", plan_path, "-o", retained, cwd=outside)
    retained_hash = sha256(retained)
    evidence.require(f"retained_exact_bytes.{name}", retained_hash == float_info["sha256"])
    return {"name": name, "source": str(original.relative_to(ROOT)), "source_hash": source_hash,
            "float32": float_info, "pcm16": pcm_info, "plan": plan_info,
            "retained_sha256": retained_hash, "copied_source_removed_before_render": True}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", default=sys.executable, metavar="INTERPRETER",
                        help="Existing interpreter with Lark/jsonschema for prerequisite spec smoke")
    args = parser.parse_args()
    evidence = Evidence()
    result: dict[str, Any] = {
        "ok": False, "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "run_id": evidence.run_id, "environment": {"python": sys.version, "platform": platform.platform()},
        "listening_review": {"status": "not_performed", "note": "Numerical verification is not listening or perceptual click validation."},
        "examples": [], "commands": evidence.commands, "checks": evidence.checks,
    }
    before: dict[str, str] | None = None
    try:
        before = source_manifest()
        result["source_manifest_before"] = before
        verify_prerequisite(evidence, args.python, result)
        evidence.require("source_snapshot_stable_after_install", source_manifest() == before)
        library_hash = f"sha256:{sha256(BASIC_SOURCE)}"
        result["basic_library_source_hash"] = library_hash
        sources = {p.stem for p in (ROOT / "examples/basic").glob("*.maac")}
        evidence.require("exactly_25_basic_examples", sources == EXPECTED_NAMES | {"full_band"})
        with tempfile.TemporaryDirectory(prefix="maac-basic-installed-") as work:
            outside = Path(work).resolve()
            evidence.require("work_is_outside_checkout", not outside.is_relative_to(ROOT.resolve()))
            result["outside_checkout_work_directory"] = str(outside)
            result["catalog"] = verify_catalog(evidence, outside, library_hash)
            verify_failures(evidence, outside)
            for name in sorted(sources):
                result["examples"].append(verify_example(evidence, outside, name, library_hash))
                print(f"Verified installed basic example: {name}", file=sys.stderr, flush=True)
        evidence.require("all_25_examples_verified", len(result["examples"]) == 25)
        result["ok"] = True
    except (GateFailure, OSError, ValueError, KeyError, TypeError) as error:
        result["failure"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        try:
            after = source_manifest()
            result["source_manifest_after"] = after
            evidence.require("final_source_snapshot_stable", before is not None and before == after)
            if evidence.binary_hash is not None:
                result["installed_binary_sha256_after"] = sha256(INSTALLED)
                evidence.require("final_installed_binary_stable", evidence.binary_hash == result["installed_binary_sha256_after"])
        except (GateFailure, OSError) as error:
            result["ok"] = False
            result["snapshot_failure"] = str(error)
        result["ok"] = result["ok"] and all(check["ok"] for check in evidence.checks)
        result["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        result["auditions"] = str(evidence.auditions.relative_to(ROOT))
        report = evidence.run / "results.json"
        payload = json.dumps(result, indent=2, sort_keys=True) + "\n"
        report.write_text(payload, encoding="utf-8")
        (ARTIFACTS / "results.json").write_text(payload, encoding="utf-8")
        print(json.dumps({"ok": result["ok"], "results": str(report.relative_to(ROOT)),
                          "auditions": str(evidence.auditions.relative_to(ROOT))}, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
