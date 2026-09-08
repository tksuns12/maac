#!/usr/bin/env python3
"""Run the offline installed MaaC CLI acceptance checks for ScoreIR.

This runner intentionally uses only the Python standard library.  It keeps
all generated files under ``target/acceptance`` and writes command output to
sidecar logs so a failed release check remains inspectable.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import re
import shlex
import struct
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "target" / "acceptance"
LOGS = ARTIFACTS / "command-logs"

BASELINE_HASHES = {
    "example.maac": "3e336b05bc1099112c1107c8aa506e2ab2e585012e12ce3415d15383565fe2ae",
    "evening-window.maac": "9a72ae3503f2bb3410963db71964240ae89c2dd5ce24a8a19d7ca8445b8a745b",
}


class Acceptance:
    def __init__(self) -> None:
        ARTIFACTS.mkdir(parents=True, exist_ok=True)
        LOGS.mkdir(parents=True, exist_ok=True)
        self.commands: list[dict[str, Any]] = []
        self.checks: list[dict[str, Any]] = []

    def check(self, name: str, passed: bool, detail: Any = None) -> bool:
        item: dict[str, Any] = {"name": name, "ok": bool(passed)}
        if detail is not None:
            item["detail"] = detail
        self.checks.append(item)
        return passed

    def command(
        self,
        name: str,
        argv: Iterable[str | Path],
        *,
        cwd: Path = ROOT,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        args = [str(value) for value in argv]
        try:
            completed = subprocess.run(
                args,
                cwd=cwd,
                env=env,
                text=True,
                capture_output=True,
                check=False,
                errors="replace",
            )
        except OSError as error:
            completed = subprocess.CompletedProcess(args, 127, "", str(error))
        safe_name = re.sub(r"[^A-Za-z0-9_.-]+", "_", name)
        stdout_path = LOGS / f"{len(self.commands):03d}-{safe_name}.stdout"
        stderr_path = LOGS / f"{len(self.commands):03d}-{safe_name}.stderr"
        stdout_path.write_text(completed.stdout, encoding="utf-8")
        stderr_path.write_text(completed.stderr, encoding="utf-8")
        self.commands.append(
            {
                "name": name,
                "argv": args,
                "command": shlex.join(args),
                "cwd": str(cwd),
                "returncode": completed.returncode,
                "ok": completed.returncode == 0,
                "stdout_log": str(stdout_path.relative_to(ROOT)),
                "stderr_log": str(stderr_path.relative_to(ROOT)),
            }
        )
        return completed


def parse_json(stdout: str) -> dict[str, Any] | None:
    try:
        value = json.loads(stdout)
    except (json.JSONDecodeError, TypeError):
        return None
    return value if isinstance(value, dict) else None


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def extract_tutorial_source(path: Path) -> str:
    source = path.read_text(encoding="utf-8")
    match = re.search(r"```maac\s*\n(.*?)\n```", source, re.DOTALL)
    if match is None:
        raise ValueError("docs/tutorial.md has no complete maac source fence")
    return match.group(1) + "\n"


def wav_info(path: Path) -> dict[str, Any]:
    """Read the small RIFF/WAVE subset emitted by hound without third parties."""

    raw = path.read_bytes()
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise ValueError("not a RIFF/WAVE file")
    offset = 12
    fmt: tuple[int, int, int, int, int, int] | None = None
    fmt_chunk: bytes | None = None
    payload = bytearray()
    while offset + 8 <= len(raw):
        chunk_id = raw[offset : offset + 4]
        size = struct.unpack_from("<I", raw, offset + 4)[0]
        start = offset + 8
        end = start + size
        if end > len(raw):
            raise ValueError("truncated WAVE chunk")
        chunk = raw[start:end]
        if chunk_id == b"fmt ":
            if len(chunk) < 16:
                raise ValueError("short WAVE fmt chunk")
            fmt = struct.unpack_from("<HHIIHH", chunk, 0)
            fmt_chunk = chunk
        elif chunk_id == b"data":
            payload.extend(chunk)
        offset = end + (size & 1)
    if fmt is None or not payload:
        raise ValueError("WAVE file has no format or data")
    audio_format, channels, sample_rate, _byte_rate, block_align, bits = fmt
    # hound writes WAVE_FORMAT_EXTENSIBLE for multi-channel output.  Its
    # first four bytes of SubFormat carry the ordinary PCM (1) or IEEE float
    # (3) tag; the remaining GUID bytes are the standard WAVE GUID suffix.
    if audio_format == 0xFFFE:
        if fmt_chunk is None or len(fmt_chunk) < 40:
            raise ValueError("short WAVE extensible fmt chunk")
        audio_format = struct.unpack_from("<I", fmt_chunk, 24)[0]
    if channels == 0 or block_align == 0 or len(payload) % block_align:
        raise ValueError("invalid WAVE channel or block alignment")
    frames = len(payload) // block_align
    if audio_format == 3 and bits == 32:
        if len(payload) % 4:
            raise ValueError("unaligned float32 WAVE payload")
        samples = (value[0] for value in struct.iter_unpack("<f", payload))
        encoding = "float32"
    elif audio_format == 1 and bits == 16:
        if len(payload) % 2:
            raise ValueError("unaligned PCM16 WAVE payload")
        samples = (value[0] for value in struct.iter_unpack("<h", payload))
        encoding = "pcm16"
    else:
        raise ValueError(
            f"unsupported WAVE format={audio_format} bits={bits} channels={channels}"
        )
    finite = True
    nonzero = 0
    peak = 0.0
    sample_count = 0
    for sample in samples:
        sample_count += 1
        numeric = float(sample)
        if not math.isfinite(numeric):
            finite = False
        peak = max(peak, abs(numeric))
        if numeric != 0.0:
            nonzero += 1
    if sample_count != frames * channels:
        raise ValueError("WAVE data length does not match frame count")
    return {
        "path": str(path.relative_to(ROOT)),
        "encoding": encoding,
        "audio_format": audio_format,
        "channels": channels,
        "sample_rate_hz": sample_rate,
        "bits_per_sample": bits,
        "frames": frames,
        "samples": sample_count,
        "finite": finite,
        "nonzero_samples": nonzero,
        "peak": peak,
        "bytes": len(raw),
    }


def check_json_result(
    acceptance: Acceptance,
    name: str,
    completed: subprocess.CompletedProcess[str],
    *,
    expected_command: str | None = None,
    expected_notes: int | None = None,
    expected_frames: int | None = None,
) -> dict[str, Any] | None:
    parsed = parse_json(completed.stdout)
    ok = completed.returncode == 0 and parsed is not None and parsed.get("ok") is True
    if expected_command is not None:
        ok = ok and parsed is not None and parsed.get("command") == expected_command
    if expected_notes is not None:
        ok = ok and parsed is not None and parsed.get("notes") == expected_notes
    if expected_frames is not None:
        ok = ok and parsed is not None and parsed.get("frames") == expected_frames
    acceptance.check(name, ok, parsed or {"returncode": completed.returncode})
    return parsed


def inspect_expected_wav(
    acceptance: Acceptance,
    name: str,
    path: Path,
    *,
    encoding: str,
    frames: int,
    channels: int = 2,
) -> dict[str, Any] | None:
    try:
        info = wav_info(path)
    except (OSError, ValueError) as error:
        acceptance.check(name, False, str(error))
        return None
    passed = (
        info["encoding"] == encoding
        and info["channels"] == channels
        and info["sample_rate_hz"] == 48_000
        and info["frames"] == frames
        and info["finite"]
        and info["nonzero_samples"] > 0
    )
    acceptance.check(name, passed, info)
    return info


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-install",
        action="store_true",
        help="use target/install/bin/maac without running cargo install",
    )
    args = parser.parse_args()

    acceptance = Acceptance()
    started = datetime.now(timezone.utc).isoformat()
    result: dict[str, Any] = {
        "started_at_utc": started,
        "root": str(ROOT),
        "artifacts": str(ARTIFACTS.relative_to(ROOT)),
        "environment": {"python": sys.version, "platform": platform.platform()},
        "commands": acceptance.commands,
        "checks": acceptance.checks,
        "source_hashes": {},
        "compositions": {},
        "tutorial": {},
        "boundaries": {},
    }

    for executable, label in (
        (["sw_vers", "-productVersion"], "sw_vers_product_version"),
        (["sw_vers", "-buildVersion"], "sw_vers_build_version"),
        (["rustc", "--version"], "rustc_version"),
        (["cargo", "--version"], "cargo_version"),
    ):
        observed = acceptance.command(label, executable)
        result["environment"][label] = {
            "returncode": observed.returncode,
            "stdout": observed.stdout.strip(),
            "stderr": observed.stderr.strip(),
        }
        acceptance.check(
            f"environment.{label}", observed.returncode == 0, result["environment"][label]
        )

    install_root = ROOT / "target" / "install"
    binary = install_root / "bin" / "maac"
    if args.skip_install:
        install = None
        install_ok = binary.is_file()
    else:
        install = acceptance.command(
            "cargo_install_offline",
            [
                "cargo",
                "install",
                "--path",
                ".",
                "--root",
                "target/install",
                "--locked",
                "--offline",
            ],
        )
        install_ok = install.returncode == 0 and binary.is_file()
        acceptance.check(
            "offline_install",
            install_ok,
            {"returncode": install.returncode, "binary": str(binary)},
        )
    result["installed_binary"] = str(binary.relative_to(ROOT))
    if args.skip_install:
        acceptance.check("installed_binary", binary.is_file(), str(binary))

    # Never run a stale executable after a failed install.  This matters when
    # another workspace agent is changing production code concurrently.
    cli_ready = install_ok
    compositions = {
        "example": {
            "source": ROOT / "example.maac",
            "notes": 48,
            "frames": 816_000,
        },
        "evening-window": {
            "source": ROOT / "evening-window.maac",
            "notes": 182,
            "frames": 1_968_000,
        },
    }

    for stem, spec in compositions.items():
        source = spec["source"]
        source_hash = sha256(source)
        expected_hash = BASELINE_HASHES[source.name]
        result["source_hashes"][source.name] = {
            "observed": source_hash,
            "baseline": expected_hash,
            "match": source_hash == expected_hash,
        }
        acceptance.check(
            f"source_hash.{source.name}", source_hash == expected_hash, result["source_hashes"][source.name]
        )
        composition_result: dict[str, Any] = {
            "source": str(source.relative_to(ROOT)),
            "expected_notes": spec["notes"],
            "expected_frames": spec["frames"],
        }
        result["compositions"][stem] = composition_result
        if not cli_ready:
            acceptance.check(f"{stem}.cli_available", False, "installed CLI is unavailable")
            continue

        plan = ARTIFACTS / f"{stem}.performance.json"
        wav = ARTIFACTS / f"{stem}.wav"
        build_wav = ARTIFACTS / f"{stem}.build.wav"
        repeat_wav = ARTIFACTS / f"{stem}.repeat.wav"
        pcm_wav = ARTIFACTS / f"{stem}.pcm16.wav"
        check = acceptance.command(
            f"{stem}_check",
            [binary, "--json", "check", source],
        )
        composition_result["check"] = check_json_result(
            acceptance,
            f"{stem}.check",
            check,
            expected_command="check",
            expected_notes=spec["notes"],
            expected_frames=spec["frames"],
        )
        compile_result = acceptance.command(
            f"{stem}_compile",
            [binary, "--json", "compile", source, "-o", plan, "--force"],
        )
        composition_result["compile"] = check_json_result(
            acceptance,
            f"{stem}.compile",
            compile_result,
            expected_command="compile",
            expected_notes=spec["notes"],
            expected_frames=spec["frames"],
        )
        render_result = acceptance.command(
            f"{stem}_render",
            [binary, "--json", "render", plan, "-o", wav, "--force"],
        )
        composition_result["render"] = check_json_result(
            acceptance,
            f"{stem}.render",
            render_result,
            expected_command="render",
            expected_frames=spec["frames"],
        )
        composition_result["wav"] = inspect_expected_wav(
            acceptance,
            f"{stem}.wav",
            wav,
            encoding="float32",
            frames=spec["frames"],
        )
        build_result = acceptance.command(
            f"{stem}_build",
            [binary, "--json", "build", source, "-o", build_wav, "--force"],
        )
        composition_result["build"] = check_json_result(
            acceptance,
            f"{stem}.build",
            build_result,
            expected_command="build",
            expected_frames=spec["frames"],
        )
        composition_result["build_wav"] = inspect_expected_wav(
            acceptance,
            f"{stem}.build_wav",
            build_wav,
            encoding="float32",
            frames=spec["frames"],
        )
        render_bytes = wav.read_bytes() if wav.is_file() else None
        build_bytes = build_wav.read_bytes() if build_wav.is_file() else None
        acceptance.check(
            f"{stem}.render_build_pcm_identical",
            render_bytes is not None and render_bytes == build_bytes,
            {"render_bytes": len(render_bytes or b""), "build_bytes": len(build_bytes or b"")},
        )
        repeat_result = acceptance.command(
            f"{stem}_repeat_render",
            [binary, "--json", "render", plan, "-o", repeat_wav, "--force"],
        )
        composition_result["repeat_render"] = check_json_result(
            acceptance,
            f"{stem}.repeat_render",
            repeat_result,
            expected_command="render",
            expected_frames=spec["frames"],
        )
        repeat_bytes = repeat_wav.read_bytes() if repeat_wav.is_file() else None
        acceptance.check(
            f"{stem}.repeat_pcm_identical",
            render_bytes is not None and render_bytes == repeat_bytes,
            {"render_bytes": len(render_bytes or b""), "repeat_bytes": len(repeat_bytes or b"")},
        )
        pcm_result = acceptance.command(
            f"{stem}_pcm16_build",
            [binary, "--json", "build", source, "-o", pcm_wav, "--format", "pcm16", "--force"],
        )
        composition_result["pcm16_build"] = check_json_result(
            acceptance,
            f"{stem}.pcm16_build",
            pcm_result,
            expected_command="build",
            expected_frames=spec["frames"],
        )
        composition_result["pcm16_wav"] = inspect_expected_wav(
            acceptance,
            f"{stem}.pcm16_wav",
            pcm_wav,
            encoding="pcm16",
            frames=spec["frames"],
        )

    tutorial_source = ARTIFACTS / "tutorial.maac"
    tutorial_plan = ARTIFACTS / "tutorial.performance.json"
    tutorial_wav = ARTIFACTS / "tutorial.wav"
    try:
        tutorial_source.write_text(extract_tutorial_source(ROOT / "docs" / "tutorial.md"), encoding="utf-8")
        result["tutorial"]["source"] = str(tutorial_source.relative_to(ROOT))
        acceptance.check("tutorial.source_fence", True, str(tutorial_source))
    except (OSError, ValueError) as error:
        acceptance.check("tutorial.source_fence", False, str(error))
    if cli_ready and tutorial_source.is_file():
        tutorial_check = acceptance.command(
            "tutorial_check", [binary, "--json", "check", tutorial_source]
        )
        result["tutorial"]["check"] = check_json_result(
            acceptance,
            "tutorial.check",
            tutorial_check,
            expected_command="check",
            expected_notes=3,
            expected_frames=144_000,
        )
        tutorial_compile = acceptance.command(
            "tutorial_compile", [binary, "--json", "compile", tutorial_source, "-o", tutorial_plan, "--force"]
        )
        result["tutorial"]["compile"] = check_json_result(
            acceptance,
            "tutorial.compile",
            tutorial_compile,
            expected_command="compile",
            expected_notes=3,
            expected_frames=144_000,
        )
        tutorial_render = acceptance.command(
            "tutorial_render", [binary, "--json", "render", tutorial_plan, "-o", tutorial_wav, "--force"]
        )
        result["tutorial"]["render"] = check_json_result(
            acceptance,
            "tutorial.render",
            tutorial_render,
            expected_command="render",
            expected_frames=144_000,
        )
        result["tutorial"]["wav"] = inspect_expected_wav(
            acceptance,
            "tutorial.wav",
            tutorial_wav,
            encoding="float32",
            frames=144_000,
        )

    if cli_ready:
        overwrite = ARTIFACTS / "overwrite-check.wav"
        overwrite.write_bytes(b"acceptance sentinel\n")
        overwrite_before = overwrite.read_bytes()
        overwrite_attempt = acceptance.command(
            "overwrite_without_force",
            [binary, "--json", "build", ROOT / "example.maac", "-o", overwrite],
        )
        overwrite_json = parse_json(overwrite_attempt.stdout)
        overwrite_ok = (
            overwrite_attempt.returncode != 0
            and overwrite_json is not None
            and overwrite_json.get("code") == "E_OUTPUT_EXISTS"
            and overwrite.read_bytes() == overwrite_before
        )
        acceptance.check("overwrite_protection", overwrite_ok, overwrite_json or {})
        overwrite_force = acceptance.command(
            "overwrite_with_force",
            [binary, "--json", "build", ROOT / "example.maac", "-o", overwrite, "--force"],
        )
        acceptance.check(
            "overwrite_force", overwrite_force.returncode == 0 and overwrite.stat().st_size > len(overwrite_before),
            parse_json(overwrite_force.stdout) or {"returncode": overwrite_force.returncode},
        )
        failed_source = ARTIFACTS / "malformed.maac"
        failed_output = ARTIFACTS / "failed-output.wav"
        failed_source.write_text("maac 1; project {", encoding="utf-8")
        if failed_output.exists():
            failed_output.unlink()
        failed = acceptance.command(
            "failed_build_no_destination",
            [binary, "--json", "build", failed_source, "-o", failed_output],
        )
        failed_json = parse_json(failed.stdout)
        acceptance.check(
            "failed_build_leaves_no_destination",
            failed.returncode != 0 and failed_json is not None and failed_json.get("ok") is False and not failed_output.exists(),
            failed_json or {"returncode": failed.returncode},
        )
        result["boundaries"]["overwrite"] = overwrite_json
        result["boundaries"]["failure"] = failed_json

    # This test target exercises the public exact/music library APIs used by
    # the reference examples without adding an acceptance-only Rust fixture.
    library = acceptance.command("library_music_api", ["cargo", "test", "--offline", "--test", "music"])
    acceptance.check("library_music_api", library.returncode == 0, {"returncode": library.returncode})
    result["library_api"] = {"command": "cargo test --offline --test music", "returncode": library.returncode}

    result["commands"] = acceptance.commands
    result["checks"] = acceptance.checks
    result["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    result["ok"] = all(item["ok"] for item in acceptance.checks)
    result_path = ARTIFACTS / "results.json"
    result_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"ok": result["ok"], "results": str(result_path.relative_to(ROOT))}, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
