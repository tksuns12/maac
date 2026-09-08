#!/usr/bin/env python3
"""Validate installed reusable-instrument delivery and compatibility boundaries.

This standard-library runner first executes the existing installed CLI acceptance
suite.  It only exercises the installed binary after that suite proves a fresh
offline install.  All files made by this runner stay below
``target/instrument-acceptance``.

The disposable specification checker needs Lark and jsonschema.  Select its
interpreter with ``--python .venv/bin/python`` when those packages are not
installed for the interpreter running this script.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import stat
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "target" / "instrument-acceptance"
LOGS = ARTIFACTS / "command-logs"
WORK = ARTIFACTS / "work"
LEGACY_ARTIFACTS = ROOT / "target" / "acceptance"
INSTALLED_BINARY = ROOT / "target" / "install" / "bin" / "maac"

LEGACY_BASELINE_ENVIRONMENT = {
    "system": "Darwin",
    "machine": "arm64",
    "macos_product_version": "26.6.2",
    "macos_build_version": "25G83",
}
LEGACY_BASELINES = (
    {
        "source": "example.maac",
        "format": "float32",
        "sha256": "0fe1e92b399697f30796af7586e8c1c3b1b82a298ebd336f92d1c57c44a471dd",
        "bytes": 6_528_068,
    },
    {
        "source": "example.maac",
        "format": "pcm16",
        "sha256": "f8e3d0017bd54c68af7c09af62fcaf8cb79869d31269b5364b99860cf4268a51",
        "bytes": 3_264_044,
    },
    {
        "source": "evening-window.maac",
        "format": "float32",
        "sha256": "0c053fd2e53e0658ec356e0336a00585522297f1f9922f0b654f870c3b886924",
        "bytes": 15_744_068,
    },
    {
        "source": "evening-window.maac",
        "format": "pcm16",
        "sha256": "58d0d6cb70100aa1bfb06cd1738ef4fd7550028426a028efe1978b83f71637c8",
        "bytes": 7_872_044,
    },
)
LEGACY_GENERATED_PATHS = tuple(
    f"{stem}{suffix}"
    for stem in ("example", "evening-window")
    for suffix in (".performance.json", ".wav", ".build.wav", ".repeat.wav", ".pcm16.wav")
) + (
    "tutorial.maac",
    "tutorial.performance.json",
    "tutorial.wav",
    "overwrite-check.wav",
    "malformed.maac",
    "failed-output.wav",
    "results.json",
)

sys.path.insert(0, str(ROOT / "scripts"))
from acceptance import parse_json, sha256, wav_info  # noqa: E402


class GateFailure(RuntimeError):
    """A required delivery gate failed; later dependent gates must not run."""


class Evidence:
    def __init__(self) -> None:
        ARTIFACTS.mkdir(parents=True, exist_ok=True)
        LOGS.mkdir(parents=True, exist_ok=True)
        self.commands: list[dict[str, Any]] = []
        self.checks: list[dict[str, Any]] = []

    def command(
        self,
        name: str,
        argv: Iterable[str | Path],
        *,
        cwd: Path = ROOT,
    ) -> subprocess.CompletedProcess[str]:
        args = [str(value) for value in argv]
        try:
            completed = subprocess.run(
                args,
                cwd=cwd,
                text=True,
                capture_output=True,
                check=False,
                errors="replace",
            )
        except OSError as error:
            completed = subprocess.CompletedProcess(args, 127, "", str(error))
        safe_name = re.sub(r"[^A-Za-z0-9_.-]+", "_", name)
        sequence = len(self.commands)
        stdout_path = LOGS / f"{sequence:03d}-{safe_name}.stdout"
        stderr_path = LOGS / f"{sequence:03d}-{safe_name}.stderr"
        stdout_path.write_text(completed.stdout, encoding="utf-8")
        stderr_path.write_text(completed.stderr, encoding="utf-8")
        self.commands.append(
            {
                "name": name,
                "argv": args,
                "command": shlex.join(args),
                "cwd": str(cwd),
                "returncode": completed.returncode,
                "stdout": completed.stdout,
                "stderr": completed.stderr,
                "stdout_log": str(stdout_path.relative_to(ROOT)),
                "stderr_log": str(stderr_path.relative_to(ROOT)),
            }
        )
        return completed

    def check(self, name: str, passed: bool, detail: Any = None) -> bool:
        item: dict[str, Any] = {"name": name, "ok": bool(passed)}
        if detail is not None:
            item["detail"] = detail
        self.checks.append(item)
        return bool(passed)

    def require(self, name: str, passed: bool, detail: Any = None) -> None:
        if not self.check(name, passed, detail):
            raise GateFailure(name)


def remove_output(path: Path) -> None:
    """Remove one known runner-owned output so stale bytes cannot satisfy a gate."""

    if path.is_dir():
        raise GateFailure(f"runner output is unexpectedly a directory: {path}")
    if path.exists() or path.is_symlink():
        path.unlink()


def preclean_legacy_artifacts(evidence: Evidence) -> None:
    """Remove only old-run destinations so legacy acceptance proves publication."""

    removed: list[str] = []
    for relative in LEGACY_GENERATED_PATHS:
        path = LEGACY_ARTIFACTS / relative
        existed = path.exists() or path.is_symlink()
        remove_output(path)
        if existed:
            try:
                removed.append(str(path.relative_to(ROOT)))
            except ValueError:
                removed.append(str(path))
    evidence.require(
        "legacy_acceptance_outputs_precleaned",
        all(
            not (LEGACY_ARTIFACTS / relative).exists()
            and not (LEGACY_ARTIFACTS / relative).is_symlink()
            for relative in LEGACY_GENERATED_PATHS
        ),
        {"removed": removed, "exact_destinations": list(LEGACY_GENERATED_PATHS)},
    )


def json_success(
    evidence: Evidence,
    name: str,
    argv: Iterable[str | Path],
    *,
    expected_command: str,
    expected_frames: int | None = None,
    expected_format: str | None = None,
    expected_exports: int | None = None,
    cwd: Path = ROOT,
) -> dict[str, Any]:
    completed = evidence.command(name, argv, cwd=cwd)
    parsed = parse_json(completed.stdout)
    passed = (
        completed.returncode == 0
        and parsed is not None
        and parsed.get("ok") is True
        and parsed.get("command") == expected_command
    )
    if expected_frames is not None:
        passed = passed and parsed is not None and parsed.get("frames") == expected_frames
    if expected_format is not None:
        passed = passed and parsed is not None and parsed.get("format") == expected_format
    if expected_exports is not None:
        passed = passed and parsed is not None and parsed.get("exports") == expected_exports
    evidence.require(
        name,
        passed,
        parsed if parsed is not None else {"returncode": completed.returncode},
    )
    assert parsed is not None
    return parsed


def file_fingerprint(path: Path) -> dict[str, Any]:
    observed = path.stat()
    return {
        "sha256": sha256(path),
        "bytes": observed.st_size,
        "mtime_ns": observed.st_mtime_ns,
        "mode": stat.S_IMODE(observed.st_mode),
        "inode": observed.st_ino,
    }


def inspect_audio(
    evidence: Evidence,
    name: str,
    path: Path,
    *,
    encoding: str,
) -> dict[str, Any]:
    try:
        info = wav_info(path)
    except (OSError, ValueError) as error:
        evidence.require(name, False, str(error))
        raise AssertionError("unreachable")
    peak_scale = 32768.0 if encoding == "pcm16" else 1.0
    info["normalized_peak"] = info["peak"] / peak_scale
    passed = (
        info["encoding"] == encoding
        and info["channels"] == 2
        and info["sample_rate_hz"] == 48_000
        and info["frames"] == 480_000
        and info["finite"]
        and info["nonzero_samples"] > 0
        and info["normalized_peak"] <= 1.0
    )
    evidence.require(name, passed, info)
    return info


def load_legacy_results(evidence: Evidence) -> dict[str, Any]:
    preclean_legacy_artifacts(evidence)
    legacy = evidence.command(
        "legacy_installed_acceptance",
        [sys.executable, ROOT / "scripts" / "acceptance.py"],
    )
    summary = parse_json(legacy.stdout)
    try:
        details = json.loads((LEGACY_ARTIFACTS / "results.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        evidence.require("legacy_installed_acceptance", False, str(error))
        raise AssertionError("unreachable")
    passed = (
        legacy.returncode == 0
        and summary is not None
        and summary.get("ok") is True
        and details.get("ok") is True
        and any(
            check.get("name") == "offline_install" and check.get("ok") is True
            for check in details.get("checks", [])
        )
        and INSTALLED_BINARY.is_file()
        and os.access(INSTALLED_BINARY, os.X_OK)
    )
    evidence.require(
        "legacy_installed_acceptance",
        passed,
        {
            "returncode": legacy.returncode,
            "summary": summary,
            "details": str((LEGACY_ARTIFACTS / "results.json").relative_to(ROOT)),
            "offline_install_proved": passed,
            "installed_binary": str(INSTALLED_BINARY.relative_to(ROOT)),
        },
    )
    return details


def verify_library_and_hash(evidence: Evidence, result: dict[str, Any]) -> None:
    library = ROOT / "examples" / "sounds" / "studio.maac"
    composition = ROOT / "examples" / "reusable.maac"
    result["library"] = json_success(
        evidence,
        "installed_library_exports",
        [INSTALLED_BINARY, "--json", "check", library],
        expected_command="check",
        expected_exports=10,
    )

    pins = re.findall(r'\bhash\s*=\s*"(sha256:[0-9a-f]{64})"', composition.read_text(encoding="utf-8"))
    evidence.require("studio_import_has_one_pin", len(pins) == 1, {"pins": pins})
    before = file_fingerprint(library)
    hashed = evidence.command(
        "installed_hash_studio",
        [INSTALLED_BINARY, "--json", "hash", library],
    )
    parsed = parse_json(hashed.stdout)
    after = file_fingerprint(library)
    expected = f"sha256:{before['sha256']}"
    passed = (
        hashed.returncode == 0
        and parsed is not None
        and parsed.get("ok") is True
        and parsed.get("command") == "hash"
        and parsed.get("digest") == expected
        and pins == [expected]
        and before == after
    )
    evidence.require(
        "installed_hash_matches_pin_and_is_read_only",
        passed,
        {"cli": parsed, "pin": pins[0] if pins else None, "before": before, "after": after},
    )
    result["hash"] = {
        "source": str(library.relative_to(ROOT)),
        "pin": pins[0],
        "digest": expected,
        "before": before,
        "after": after,
    }


def build_reusable(evidence: Evidence, result: dict[str, Any]) -> bytes:
    source = ROOT / "examples" / "reusable.maac"
    float_wav = WORK / "reusable.float32.wav"
    repeat_wav = WORK / "reusable.repeat.float32.wav"
    pcm_wav = WORK / "reusable.pcm16.wav"
    for path in (float_wav, repeat_wav, pcm_wav):
        remove_output(path)

    float_result = json_success(
        evidence,
        "installed_build_reusable_float32",
        [INSTALLED_BINARY, "--json", "build", source, "-o", float_wav],
        expected_command="build",
        expected_frames=480_000,
        expected_format="float32",
    )
    pcm_result = json_success(
        evidence,
        "installed_build_reusable_pcm16",
        [INSTALLED_BINARY, "--json", "build", source, "-o", pcm_wav, "--format", "pcm16"],
        expected_command="build",
        expected_frames=480_000,
        expected_format="pcm16",
    )
    repeat_result = json_success(
        evidence,
        "installed_repeat_reusable_float32",
        [INSTALLED_BINARY, "--json", "build", source, "-o", repeat_wav],
        expected_command="build",
        expected_frames=480_000,
        expected_format="float32",
    )
    float_info = inspect_audio(evidence, "reusable_float32_audio", float_wav, encoding="float32")
    pcm_info = inspect_audio(evidence, "reusable_pcm16_audio", pcm_wav, encoding="pcm16")
    first = float_wav.read_bytes()
    repeated = repeat_wav.read_bytes()
    evidence.require(
        "reusable_float32_repeat_exact_bytes",
        first == repeated,
        {"first_sha256": sha256(float_wav), "repeat_sha256": sha256(repeat_wav), "bytes": len(first)},
    )
    result["reusable_build"] = {
        "float32": {"cli": float_result, "wav": float_info, "sha256": sha256(float_wav)},
        "pcm16": {"cli": pcm_result, "wav": pcm_info, "sha256": sha256(pcm_wav)},
        "repeat": {"cli": repeat_result, "sha256": sha256(repeat_wav)},
    }
    return first


def verify_retained_v2_plan(
    evidence: Evidence,
    result: dict[str, Any],
    expected_float_bytes: bytes,
) -> None:
    bundle = WORK / "disposable-bundle"
    if bundle.exists():
        shutil.rmtree(bundle)
    (bundle / "sounds").mkdir(parents=True)
    copied_sources = [
        (ROOT / "examples" / "reusable.maac", bundle / "reusable.maac"),
        (ROOT / "examples" / "sounds" / "studio.maac", bundle / "sounds" / "studio.maac"),
        (ROOT / "examples" / "sounds" / "colors.wav", bundle / "sounds" / "colors.wav"),
    ]
    for source, destination in copied_sources:
        shutil.copy2(source, destination)

    plan = WORK / "reusable.v2.performance.json"
    retained_wav = WORK / "reusable.retained-plan.float32.wav"
    malformed_plan = WORK / "reusable.malformed-v2.performance.json"
    malformed_output = WORK / "reusable.malformed-v2.wav"
    for path in (plan, retained_wav, malformed_plan, malformed_output):
        remove_output(path)

    compile_result = json_success(
        evidence,
        "installed_compile_disposable_reusable_v2",
        [INSTALLED_BINARY, "--json", "compile", bundle / "reusable.maac", "-o", plan],
        expected_command="compile",
        expected_frames=480_000,
    )
    plan_document = json.loads(plan.read_text(encoding="utf-8"))
    evidence.require(
        "compiled_plan_is_self_contained_v2",
        plan_document.get("version") == 2 and isinstance(plan_document.get("instruments"), dict),
        {"version": plan_document.get("version"), "has_instruments": isinstance(plan_document.get("instruments"), dict)},
    )

    removed: list[str] = []
    for _source, copied in copied_sources:
        copied.unlink()
        removed.append(str(copied.relative_to(ROOT)))
    evidence.require(
        "copied_sources_and_assets_removed",
        all(not copied.exists() for _source, copied in copied_sources),
        {"removed": removed},
    )

    retained_result = json_success(
        evidence,
        "installed_render_retained_v2_plan",
        [INSTALLED_BINARY, "--json", "render", plan, "-o", retained_wav],
        expected_command="render",
        expected_frames=480_000,
        expected_format="float32",
    )
    retained_info = inspect_audio(
        evidence,
        "retained_v2_plan_audio",
        retained_wav,
        encoding="float32",
    )
    retained_bytes = retained_wav.read_bytes()
    evidence.require(
        "retained_v2_plan_matches_source_build_exactly",
        retained_bytes == expected_float_bytes,
        {"retained_sha256": sha256(retained_wav), "bytes": len(retained_bytes)},
    )

    malformed = json.loads(json.dumps(plan_document))
    malformed["instruments"]["entry_source"] = "../escape.maac"
    malformed_plan.write_text(json.dumps(malformed, separators=(",", ":")) + "\n", encoding="utf-8")
    rejected = evidence.command(
        "installed_reject_malformed_v2",
        [INSTALLED_BINARY, "--json", "render", malformed_plan, "-o", malformed_output],
    )
    rejection = parse_json(rejected.stdout)
    evidence.require(
        "malformed_v2_rejected_without_output",
        rejected.returncode != 0
        and rejection is not None
        and rejection.get("ok") is False
        and not malformed_output.exists(),
        {"returncode": rejected.returncode, "error": rejection, "output_exists": malformed_output.exists()},
    )
    result["retained_v2"] = {
        "compile": compile_result,
        "removed_originals": removed,
        "render": retained_result,
        "wav": retained_info,
        "sha256": sha256(retained_wav),
        "malformed_rejection": rejection,
    }


def verify_legacy_hashes(
    evidence: Evidence,
    result: dict[str, Any],
    legacy_results: dict[str, Any],
) -> None:
    legacy_environment = legacy_results.get("environment", {})
    current_environment = {
        "system": platform.system(),
        "machine": platform.machine(),
        "macos_product_version": legacy_environment.get("sw_vers_product_version", {}).get("stdout"),
        "macos_build_version": legacy_environment.get("sw_vers_build_version", {}).get("stdout"),
    }
    environment_matches = current_environment == LEGACY_BASELINE_ENVIRONMENT
    evidence.require(
        "legacy_baseline_environment_qualified",
        environment_matches,
        {
            "reference": LEGACY_BASELINE_ENVIRONMENT,
            "current": current_environment,
            "note": "Legacy WAV hashes are only asserted in their recorded Darwin arm64 environment.",
        },
    )
    successful_commands = {
        command.get("name")
        for command in legacy_results.get("commands", [])
        if command.get("returncode") == 0
    }
    observed: list[dict[str, Any]] = []
    for item in LEGACY_BASELINES:
        source = Path(item["source"]).stem
        encoding = item["format"]
        if encoding == "float32":
            artifact = LEGACY_ARTIFACTS / f"{source}.build.wav"
            command_name = f"{source}_build"
        else:
            artifact = LEGACY_ARTIFACTS / f"{source}.pcm16.wav"
            command_name = f"{source}_pcm16_build"
        digest = sha256(artifact) if artifact.is_file() else None
        detail = {
            "source": item["source"],
            "format": encoding,
            "artifact": str(artifact.relative_to(ROOT)),
            "producer_command": command_name,
            "expected_sha256": item["sha256"],
            "observed_sha256": digest,
            "expected_bytes": item["bytes"],
            "observed_bytes": artifact.stat().st_size if artifact.is_file() else None,
        }
        evidence.require(
            f"legacy_bytes.{source}.{encoding}",
            command_name in successful_commands
            and digest == item["sha256"]
            and artifact.stat().st_size == item["bytes"],
            detail,
        )
        observed.append(detail)
    evidence.require("all_four_legacy_baselines_checked", len(observed) == 4, {"count": len(observed)})
    result["legacy_baselines"] = {
        "source": "embedded pre-change measurements",
        "reference_environment": LEGACY_BASELINE_ENVIRONMENT,
        "current_environment": current_environment,
        "environment_qualified": environment_matches,
        "outputs": observed,
    }


def resolve_python(value: str) -> Path:
    candidate = Path(value).expanduser()
    if candidate.parent == Path("."):
        located = shutil.which(value)
        if located is not None:
            candidate = Path(located)
    if not candidate.is_absolute():
        candidate = Path.cwd() / candidate
    return candidate.absolute()


def verify_python_smoke(
    evidence: Evidence,
    result: dict[str, Any],
    python: Path,
) -> None:
    evidence.require(
        "checker_python_available",
        python.is_file() and os.access(python, os.X_OK),
        {"python": str(python)},
    )
    smoke = WORK / "python-smoke"
    if smoke.exists():
        shutil.rmtree(smoke)
    smoke.mkdir(parents=True)
    for name in ("check_spec.py", "grammar.lark", "syntax-tree.schema.json", "example.maac"):
        shutil.copy2(ROOT / name, smoke / name)

    packages = evidence.command(
        "python_smoke_environment",
        [
            python,
            "-c",
            "import json,platform; from importlib.metadata import version; "
            "print(json.dumps({'python':platform.python_version(),'lark':version('lark'),'jsonschema':version('jsonschema')}))",
        ],
        cwd=smoke,
    )
    package_versions = parse_json(packages.stdout)
    evidence.require(
        "python_smoke_dependencies",
        packages.returncode == 0 and package_versions is not None,
        package_versions if package_versions is not None else {"returncode": packages.returncode},
    )
    smoke_run = evidence.command("python_disposable_check_spec", [python, "check_spec.py"], cwd=smoke)
    evidence.require(
        "python_disposable_check_spec",
        smoke_run.returncode == 0,
        {"returncode": smoke_run.returncode, "stdout": smoke_run.stdout, "stderr": smoke_run.stderr},
    )
    comparisons: list[dict[str, Any]] = []
    for name in ("check-results.json", "conformance.json", "example.syntax.json"):
        generated = smoke / name
        tracked = ROOT / name
        generated_bytes = generated.read_bytes() if generated.is_file() else None
        tracked_bytes = tracked.read_bytes()
        detail = {
            "name": name,
            "generated_sha256": hashlib.sha256(generated_bytes).hexdigest() if generated_bytes is not None else None,
            "tracked_sha256": hashlib.sha256(tracked_bytes).hexdigest(),
            "bytes": len(generated_bytes) if generated_bytes is not None else None,
        }
        evidence.require(
            f"python_reference_bytes.{name}",
            generated_bytes is not None and generated_bytes == tracked_bytes,
            detail,
        )
        comparisons.append(detail)
    result["python_smoke"] = {
        "interpreter": str(python),
        "environment": package_versions,
        "disposable_root": str(smoke.relative_to(ROOT)),
        "comparisons": comparisons,
        "tracked_outputs_written": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--python",
        default=sys.executable,
        metavar="INTERPRETER",
        help=(
            "Python interpreter with lark and jsonschema for the disposable "
            "check_spec.py smoke test (default: this interpreter)"
        ),
    )
    args = parser.parse_args()
    checker_python = resolve_python(args.python)
    evidence = Evidence()
    WORK.mkdir(parents=True, exist_ok=True)
    result: dict[str, Any] = {
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "root": str(ROOT),
        "artifacts": str(ARTIFACTS.relative_to(ROOT)),
        "environment": {
            "python": sys.version,
            "platform": platform.platform(),
            "executable": sys.executable,
            "checker_python": str(checker_python),
        },
        "commands": evidence.commands,
        "checks": evidence.checks,
        "risk": "high",
        "execution_fallback": "Sol High delivery validation; Astra unavailable",
    }
    exit_code = 1
    try:
        revision = evidence.command("git_revision", ["git", "rev-parse", "HEAD"])
        evidence.require(
            "git_revision",
            revision.returncode == 0 and bool(revision.stdout.strip()),
            {"returncode": revision.returncode, "revision": revision.stdout.strip()},
        )
        result["revision"] = revision.stdout.strip()
        legacy_results = load_legacy_results(evidence)
        result["legacy_acceptance"] = {
            "results": str((LEGACY_ARTIFACTS / "results.json").relative_to(ROOT)),
            "ok": legacy_results.get("ok") is True,
        }
        verify_library_and_hash(evidence, result)
        expected_float_bytes = build_reusable(evidence, result)
        verify_retained_v2_plan(evidence, result, expected_float_bytes)
        verify_legacy_hashes(evidence, result, legacy_results)
        verify_python_smoke(evidence, result, checker_python)
        result["ok"] = all(check["ok"] for check in evidence.checks)
        exit_code = 0 if result["ok"] else 1
    except (GateFailure, OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
        result["ok"] = False
        result["failure"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        result["commands"] = evidence.commands
        result["checks"] = evidence.checks
        result["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        result_path = ARTIFACTS / "results.json"
        result_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps({"ok": result["ok"], "results": str(result_path.relative_to(ROOT))}, sort_keys=True))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
