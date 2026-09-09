#!/usr/bin/env python3
"""Exercise real installed-CLI production delivery; standard library only.

A fresh offline install and every generated artifact stay in a unique directory
below target/production-acceptance. Failed delivery limits are expected evidence,
not a failed acceptance gate. This runner does not certify an audio meter or
replace official ITU/EBU or listening acceptance.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "target" / "production-acceptance"


class GateFailure(RuntimeError):
    pass


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return "sha256:" + h.hexdigest()


def wav_metadata(path: Path) -> dict:
    """Independently reopen RIFF bytes, decode PCM24 too, and hash data chunks."""
    total = path.stat().st_size
    fmt = None
    data = None
    with path.open("rb") as source:
        header = source.read(12)
        if len(header) != 12 or header[:4] != b"RIFF" or header[8:] != b"WAVE":
            raise GateFailure(f"invalid WAV: {path}")
        if struct.unpack_from("<I", header, 4)[0] + 8 != total:
            raise GateFailure(f"incorrect RIFF size: {path}")
        while source.tell() + 8 <= total:
            chunk, size = struct.unpack("<4sI", source.read(8))
            start = source.tell()
            if start + size > total:
                raise GateFailure(f"truncated WAV chunk: {path}")
            if chunk == b"fmt ":
                if fmt is not None or not 16 <= size <= 1024:
                    raise GateFailure("invalid format chunk")
                raw = source.read(size)
                kind, channels, rate, _, align, bits = struct.unpack_from("<HHIIHH", raw)
                if kind == 0xFFFE:
                    if len(raw) < 40:
                        raise GateFailure("short extensible WAV format")
                    kind = struct.unpack_from("<I", raw, 24)[0]
                fmt = (kind, channels, rate, align, bits)
            if chunk == b"data":
                if data is not None:
                    raise GateFailure("duplicate data chunk")
                data = (start, size)
            source.seek(start + size + (size & 1))
        if fmt is None or data is None:
            raise GateFailure("missing WAV format/data")
        kind, channels, rate, align, bits = fmt
        width = bits // 8
        if (kind, bits) not in ((1, 16), (1, 24), (3, 32)) or channels not in (1, 2):
            raise GateFailure("unsupported production WAV format")
        if align != channels * width or data[1] % align:
            raise GateFailure("invalid interleaved WAV alignment")
        source.seek(data[0])
        remaining = data[1]
        pcm_hash = hashlib.sha256()
        peak = 0.0
        nonzero = 0
        while remaining:
            block = source.read(min(4096 * align, remaining))
            if not block:
                raise GateFailure("short WAV payload")
            remaining -= len(block)
            pcm_hash.update(block)
            for offset in range(0, len(block), width):
                sample = (struct.unpack_from("<f", block, offset)[0] if kind == 3 else
                          int.from_bytes(block[offset:offset + width], "little", signed=True) / (1 << (bits - 1)))
                if not math.isfinite(sample):
                    raise GateFailure("nonfinite final encoded sample")
                peak = max(peak, abs(sample))
                nonzero += sample != 0
    return {"bytes": total, "rate": rate, "channels": channels, "bits": bits,
            "encoding": {16: "wav_pcm16le", 24: "wav_pcm24le", 32: "wav_f32le"}[bits],
            "frames": data[1] // align, "file_hash": sha256(path),
            "pcm_hash": "sha256:" + pcm_hash.hexdigest(), "sample_peak_amplitude": peak,
            "nonzero_samples": nonzero}


class Evidence:
    def __init__(self, run: Path):
        self.run = run
        self.commands: list[dict] = []
        self.checks: list[dict] = []
        self.deliveries: dict[str, dict] = {}
        (run / "logs").mkdir()

    def require(self, name: str, condition: bool, detail=None):
        self.checks.append({"name": name, "ok": bool(condition), "detail": detail})
        if not condition:
            raise GateFailure(name)

    def command(self, name: str, args, *, cwd=ROOT):
        args = [str(arg) for arg in args]
        print(f"[{name}] running", flush=True)
        start = time.monotonic()
        result = subprocess.run(args, cwd=cwd, capture_output=True, text=True, errors="replace", check=False)
        prefix = self.run / "logs" / f"{len(self.commands):02d}-{name}"
        prefix.with_suffix(".stdout").write_text(result.stdout)
        prefix.with_suffix(".stderr").write_text(result.stderr)
        elapsed = time.monotonic() - start
        self.commands.append({"name": name, "argv": args, "cwd": str(cwd), "returncode": result.returncode,
                              "elapsed_seconds": elapsed, "stdout_log": str(prefix.with_suffix('.stdout')),
                              "stderr_log": str(prefix.with_suffix('.stderr'))})
        print(f"[{name}] exit {result.returncode}, {elapsed:.2f}s", flush=True)
        return result

    def json_command(self, name: str, args):
        result = self.command(name, args)
        try:
            value = json.loads(result.stdout)
        except ValueError as error:
            raise GateFailure(f"{name}: invalid CLI JSON: {result.stdout[:500]}") from error
        self.require(name + "-json", isinstance(value, dict), value)
        return result, value


def snapshot(directory: Path) -> dict:
    return {p.name: {"hash": sha256(p), "size": p.stat().st_size, "mtime_ns": p.stat().st_mtime_ns}
            for p in sorted(directory.iterdir()) if p.is_file()}


def inspect_delivery(evidence: Evidence, name: str, result, value: dict, directory: Path,
                     delivery: str, frames: int, expected_ids: set[str], failed: bool) -> dict:
    manifest = value.get("delivery")
    evidence.require(name + "-result", isinstance(manifest, dict) and value.get("command") == "deliver"
                     and value.get("ok") is (not failed) and (result.returncode != 0) == failed, value)
    assert isinstance(manifest, dict)
    evidence.require(name + "-completion", manifest["artifact_status"] == "complete"
                     and manifest["manifest_status"] == "complete" and manifest["frames"] == frames
                     and manifest["engine_frames"] == 336000 and manifest["engine_rate"] == 48000
                     and (manifest["check_status"] == "fail") == failed
                     and set(manifest["targets"]) == expected_ids, manifest)
    published = directory / manifest["manifest_file"]
    evidence.require(name + "-published-manifest", published.is_file()
                     and json.loads(published.read_text()) == manifest)
    evidence.require(name + "-published-files", {p.name for p in directory.iterdir()} ==
                     {manifest["manifest_file"], *(target["filename"] for target in manifest["targets"].values())})
    files = {}
    for target_id, target in manifest["targets"].items():
        metadata = wav_metadata(directory / target["filename"])
        expected_bits = 32 if delivery == "archive" else (16 if target_id == "master" else 24)
        evidence.require(name + "-" + target_id + "-audio", target["artifact_status"] == "complete"
                         and target["analysis_status"] == "complete"
                         and metadata["bits"] == expected_bits and metadata["channels"] == 2
                         and metadata["frames"] == frames and metadata["nonzero_samples"] > 0
                         and all(metadata[k] == target[k] for k in ("rate", "channels", "encoding", "file_hash", "pcm_hash")), metadata)
        measurements = target["measurements"]
        evidence.require(name + "-" + target_id + "-final-measurement",
                         measurements["frames"] == frames and measurements["rate"] == metadata["rate"]
                         and measurements["channels"] == metadata["channels"]
                         and measurements["sample_peak"]["amplitude"] == metadata["sample_peak_amplitude"]
                         and measurements["true_peak"]["amplitude"] >= metadata["sample_peak_amplitude"], measurements)
        files[target_id] = metadata
    evidence.deliveries[name] = {"directory": str(directory), "manifest": manifest, "files": files}
    return files


def run_acceptance(evidence: Evidence, profile: str):
    for name, flags in [("coefficient-guards", []), ("coefficient-guards-optimized", ["-O"])]:
        result = evidence.command(name, [sys.executable, *flags, ROOT / "scripts" / "production_src_coefficients.py", "--self-test"])
        evidence.require(name, result.returncode == 0, result.stderr)
    install = evidence.run / "install"
    result = evidence.command("offline-install", ["cargo", "install", "--path", ROOT, "--root", install, "--locked", "--offline", "--force"])
    evidence.require("fresh-offline-install", result.returncode == 0, result.stderr)
    binary = install / "bin" / "maac"
    evidence.require("installed-binary", binary.is_file(), {"sha256": sha256(binary)})
    package = Path(tempfile.mkdtemp(prefix="source-package-", dir=evidence.run))
    (package / "examples").mkdir()
    source = package / "examples" / "production.maac"
    shutil.copyfile(ROOT / "examples" / "production.maac", source)
    shutil.copyfile(ROOT / "production.schema.json", package / "production.schema.json")
    plan = evidence.run / "production.performance.json"
    cd_ids = {"master", "processed_bass", "pulse"}
    archive_ids = {"master", "bass_with_room", "pulse"}

    def deliver(name, input_path, delivery, ids, *, selected=(), force=False, directory=None, original=False, failed=False):
        directory = directory or evidence.run / name
        args = [binary, "--json", "deliver", input_path, "--delivery", delivery,
                "--output-dir", directory, "--profile", profile]
        if original:
            args += ["--project-root", package]
        for target in selected:
            args += ["--target", target]
        if force:
            args += ["--force"]
        result, value = evidence.json_command(name, args)
        return inspect_delivery(evidence, name, result, value, directory, delivery,
                                308700 if delivery == "release_cd" else 672000, set(ids), failed)

    try:
        result, value = evidence.json_command("compile-retained", [binary, "--json", "compile", source, "--output", plan,
                                                                     "--project-root", package, "--profile", profile])
        evidence.require("compile-retained", result.returncode == 0 and value.get("ok") is True and plan.is_file(), value)
        cd = deliver("source-cd", source, "release_cd", cd_ids, original=True)
        archive = deliver("source-archive", source, "archive", archive_ids, original=True)
        # The unchanged example currently passes its illustrative limits.
        # A separate caller policy changes only a limit, never the audio graph.
        original_text = source.read_text()
        old = "min = -24; max = -22;"
        evidence.require("failure-policy-only-edit", original_text.count(old) == 1)
        failure_source = source.with_name("failed-policy.maac")
        failure_source.write_text(original_text.replace(old, "min = -20; max = -18;"))
        failure_plan = evidence.run / "failed-policy.performance.json"
        result, value = evidence.json_command("compile-failure-policy", [binary, "--json", "compile", failure_source,
                                              "--output", failure_plan, "--project-root", package, "--profile", profile])
        evidence.require("compile-failure-policy", result.returncode == 0 and value.get("ok") is True and failure_plan.is_file(), value)
        failed_cd = deliver("failed-cd", failure_source, "release_cd", cd_ids, original=True, failed=True)
        evidence.require("failed-limits-retain-identical-audio", failed_cd == cd)
    finally:
        shutil.rmtree(package)
    evidence.require("source-package-removed", not package.exists())
    retained_cd = deliver("retained-cd", plan, "release_cd", cd_ids)
    retained_archive = deliver("retained-archive", plan, "archive", archive_ids)
    for name, first, second in [("cd", cd, retained_cd), ("archive", archive, retained_archive)]:
        evidence.require(name + "-source-free-byte-replay", first == second)
    failed_replay = deliver("retained-failed-cd", failure_plan, "release_cd", cd_ids, failed=True)
    evidence.require("failed-policy-source-free-byte-replay", failed_replay == failed_cd)
    solo = deliver("solo-bass", plan, "release_cd", {"processed_bass"}, selected=["processed_bass"])
    evidence.require("solo-stem-preserves-sidechain-context", solo["processed_bass"] == cd["processed_bass"])
    reverse = deliver("reverse-selection", plan, "release_cd", cd_ids, selected=["pulse", "processed_bass", "master"])
    evidence.require("selection-order-byte-identity", reverse == cd)
    existing = evidence.run / "failed-cd"
    before = snapshot(existing)
    result, value = evidence.json_command("refuse-overwrite", [binary, "--json", "deliver", failure_plan, "--delivery", "release_cd",
                                                                  "--output-dir", existing, "--profile", profile])
    evidence.require("no-overwrite-preserves-all-files", result.returncode != 0 and value.get("code") == "E_OUTPUT_EXISTS"
                     and before == snapshot(existing), value)
    forced = deliver("force-replay", failure_plan, "release_cd", cd_ids, directory=existing, force=True, failed=True)
    evidence.require("force-replay-byte-identity", forced == cd)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("default", "song"), default="song")
    args = parser.parse_args()
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=ARTIFACTS))
    evidence = Evidence(run)
    failure = None
    start = datetime.now(timezone.utc).isoformat()
    try:
        run_acceptance(evidence, args.profile)
    except (GateFailure, OSError, ValueError, KeyError) as error:
        failure = str(error)
        print(f"Acceptance failed: {failure}", file=sys.stderr, flush=True)
    report = {"status": "pass" if failure is None else "fail", "started_utc": start,
              "finished_utc": datetime.now(timezone.utc).isoformat(), "profile": args.profile,
              "environment": {"system": platform.system(), "machine": platform.machine(), "platform": platform.platform(),
                              "python": platform.python_version()}, "run_directory": str(run),
              "scope": "real installed CLI delivery; no official meter or listening conformance claim",
              "metering_evidence": "docs/production-metering-evidence.md",
              "source_hash": sha256(ROOT / "examples" / "production.maac"),
              "schema_hash": sha256(ROOT / "production.schema.json"),
              "commands": evidence.commands, "checks": evidence.checks, "deliveries": evidence.deliveries,
              "failure": failure}
    text = json.dumps(report, indent=2, allow_nan=False) + "\n"
    (run / "results.json").write_text(text)
    (ARTIFACTS / "results.json").write_text(text)
    print(f"{report['status']}: {run / 'results.json'}", flush=True)
    return int(failure is not None)


if __name__ == "__main__":
    raise SystemExit(main())
