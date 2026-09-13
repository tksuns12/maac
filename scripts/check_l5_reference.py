#!/usr/bin/env python3
"""Generate or verify the fixed L5 Core Audio rational reference corpus."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import sys
from fractions import Fraction
from pathlib import Path
from typing import NoReturn


REFERENCE_FORMAT = "maac.l5.sample-intervals"
MANIFEST_FORMAT = "maac.l5.core-audio-reference"
POLICY_ID = "maac.core-audio.reference-f64/1"
REFERENCE_PATH = "references/sample-intervals.json"
ASSET_PATH = "assets/impulse.pcm"
DIGITS = 90
SCALE = 10**DIGITS
MAX_WIDTH = Fraction(1, 10**80)
EPSILON = Fraction(1, 100_000_000_000_000)
Interval = tuple[Fraction, Fraction]


class CorpusError(Exception):
    """A deterministic fixed-corpus verification failure."""


CASES = (
    {
        "id": "sine-6000",
        "source": "sources/sine-6000.maac",
        "source_sha256": "sha256:3bd58a04c431e726649a78b62cd08cdafe32bb10fb8a324bdcca4f4a9228d588",
        "source_bytes": "504",
        "assets": [],
        "output_port": ["sine", "out"],
        "channels": "1",
        "channel_order": ["0"],
        "score_frames": "8",
        "tail_frames": "0",
        "timing": {"kind": "note_gate", "on_frame": "0", "off_frame": "8"},
    },
    {
        "id": "pan-center",
        "source": "sources/pan-center.maac",
        "source_sha256": "sha256:2db6f8c9d37e6b2376246914d25672282653324de79179af09fc7ca5d4245279",
        "source_bytes": "604",
        "assets": [],
        "output_port": ["pan", "out"],
        "channels": "2",
        "channel_order": ["0", "1"],
        "score_frames": "8",
        "tail_frames": "0",
        "timing": {"kind": "note_gate", "on_frame": "0", "off_frame": "8"},
    },
    {
        "id": "pan-half",
        "source": "sources/pan-half.maac",
        "source_sha256": "sha256:451e4170ece06acf224c76a50f503de91e992b9a9ec30aa4f6347fd600cd5575",
        "source_bytes": "606",
        "assets": [],
        "output_port": ["pan", "out"],
        "channels": "2",
        "channel_order": ["0", "1"],
        "score_frames": "8",
        "tail_frames": "0",
        "timing": {"kind": "note_gate", "on_frame": "0", "off_frame": "8"},
    },
    {
        "id": "one-pole-impulse",
        "source": "sources/one-pole-impulse.maac",
        "source_sha256": "sha256:dfc9d491ba2595b215d3cf81d8e7ea65ba96b9effb2e4a0243555bcfa17887a3",
        "source_bytes": "670",
        "assets": [ASSET_PATH],
        "output_port": ["filter", "out"],
        "channels": "1",
        "channel_order": ["0"],
        "score_frames": "8",
        "tail_frames": "0",
        "timing": {"kind": "audio_transport", "start_frame": "0", "end_frame": "8"},
    },
    {
        "id": "delay-feedback",
        "source": "sources/delay-feedback.maac",
        "source_sha256": "sha256:c8709cc5d62ff6453e150afd7194aae27687912e55ab47b37fe6aca7ddfcb98b",
        "source_bytes": "953",
        "assets": [],
        "output_port": ["sum", "out"],
        "channels": "1",
        "channel_order": ["0"],
        "score_frames": "4",
        "tail_frames": "4",
        "timing": {"kind": "note_gate", "on_frame": "0", "off_frame": "4"},
    },
    {
        "id": "noise-seed7",
        "source": "sources/noise-seed7.maac",
        "source_sha256": "sha256:ac2c6bc0765ddd004af74c427b7f9ffaa36c67f5ca8361916bd190b8a3c5776f",
        "source_bytes": "297",
        "assets": [],
        "output_port": ["noise", "out"],
        "channels": "1",
        "channel_order": ["0"],
        "score_frames": "8",
        "tail_frames": "0",
        "timing": {"kind": "empty_events", "events": []},
    },
)
ASSET_SHA256 = "sha256:66b7057e198043f4acfb86f84717e34610811559714e63c9484bde10fb7cbe06"
CALIBRATION_SUMMARY_SHA256 = "sha256:40f2c14088f6be43e134a5e5d25bda2067df3df2fdb674ea83134de1c72cdb27"


def fail(message: str) -> NoReturn:
    raise CorpusError(message)


def hash_uri(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def fraction_text(value: Fraction) -> str:
    return f"{value.numerator}/{value.denominator}"


def interval_json(value: Interval) -> dict[str, str]:
    return {"lo": fraction_text(value[0]), "hi": fraction_text(value[1])}


def exact(value: int | Fraction) -> Interval:
    fraction = Fraction(value)
    return fraction, fraction


def add(a: Interval, b: Interval) -> Interval:
    return a[0] + b[0], a[1] + b[1]


def mul(a: Interval, b: Interval) -> Interval:
    products = (a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1])
    return min(products), max(products)


def scale(a: Interval, value: Fraction) -> Interval:
    return mul(a, exact(value))


def negate(a: Interval) -> Interval:
    return -a[1], -a[0]


def sqrt_floor(value: Fraction) -> Fraction:
    scaled_floor = value.numerator * SCALE * SCALE // value.denominator
    return Fraction(math.isqrt(scaled_floor), SCALE)


def sqrt_interval(value: Interval) -> Interval:
    lo, hi = value
    lower = sqrt_floor(lo)
    upper_floor = sqrt_floor(hi)
    upper = upper_floor if upper_floor * upper_floor == hi else upper_floor + Fraction(1, SCALE)
    if lower * lower > lo or upper * upper < hi:
        fail("internal square-root enclosure failure")
    return lower, upper


def outward(value: Interval, digits: int) -> Interval:
    decimal_scale = 10**digits
    lo, hi = value
    lower = Fraction(lo.numerator * decimal_scale // lo.denominator, decimal_scale)
    upper_numerator = -((-hi.numerator * decimal_scale) // hi.denominator)
    upper = Fraction(upper_numerator, decimal_scale)
    if not lower <= lo <= hi <= upper:
        fail("internal outward-rounding failure")
    return lower, upper


def atan_bounds(reciprocal: int, even_terms: int = 120) -> Interval:
    def partial(last: int) -> Fraction:
        return sum(
            (
                Fraction(
                    1 if index % 2 == 0 else -1,
                    (2 * index + 1) * reciprocal ** (2 * index + 1),
                )
                for index in range(last + 1)
            ),
            Fraction(0),
        )

    lower = partial(even_terms + 1)
    upper = partial(even_terms)
    if lower > upper:
        fail("internal arctangent enclosure failure")
    return lower, upper


def exp_negative_bounds_exact(value: Fraction, even_terms: int = 120) -> Interval:
    term = Fraction(1)
    total = term
    saved = {0: total}
    for index in range(1, even_terms + 2):
        term *= -value / index
        total += term
        if index in {even_terms, even_terms + 1}:
            saved[index] = total
    lower, upper = saved[even_terms + 1], saved[even_terms]
    if lower > upper:
        fail("internal exponential enclosure failure")
    return lower, upper


def exp_negative_interval(value: Interval) -> Interval:
    # exp(-x) decreases, so use the upper x bound for the result's lower bound.
    lower = exp_negative_bounds_exact(value[1])[0]
    upper = exp_negative_bounds_exact(value[0])[1]
    return lower, upper


def derive_references() -> dict[str, list[list[Interval]]]:
    sqrt2 = sqrt_interval(exact(2))
    half_sqrt2 = scale(sqrt2, Fraction(1, 2))
    left_half = scale(
        sqrt_interval((Fraction(2) - sqrt2[1], Fraction(2) - sqrt2[0])),
        Fraction(1, 2),
    )
    right_half = scale(
        sqrt_interval((Fraction(2) + sqrt2[0], Fraction(2) + sqrt2[1])),
        Fraction(1, 2),
    )
    zero = exact(0)
    sine = [
        zero,
        half_sqrt2,
        exact(1),
        half_sqrt2,
        zero,
        negate(half_sqrt2),
        exact(-1),
        negate(half_sqrt2),
    ]
    sine_attack = [
        zero,
        scale(half_sqrt2, Fraction(1, 2)),
        exact(1),
        half_sqrt2,
        zero,
        negate(half_sqrt2),
        exact(-1),
        negate(half_sqrt2),
    ]
    center = [[mul(sample, half_sqrt2), mul(sample, half_sqrt2)] for sample in sine]
    half = [[mul(sample, left_half), mul(sample, right_half)] for sample in sine]

    atan5 = atan_bounds(5)
    atan239 = atan_bounds(239)
    pi_interval = outward(
        (
            16 * atan5[0] - 4 * atan239[1],
            16 * atan5[1] - 4 * atan239[0],
        ),
        100,
    )
    coefficient = outward(
        exp_negative_interval(scale(pi_interval, Fraction(1, 4))),
        80,
    )
    one_pole = [(Fraction(1) - coefficient[1], Fraction(1) - coefficient[0])]
    for _ in range(7):
        one_pole.append(mul(one_pole[-1], coefficient))

    feedback_input = sine[:4] + [zero] * 4
    feedback: list[Interval] = []
    previous = zero
    for sample in feedback_input:
        current = add(sample, scale(previous, Fraction(1, 2)))
        feedback.append(current)
        previous = current

    noise: list[Interval] = []
    prefix = b"maac-noise-1\0" + (7).to_bytes(8, "little") + b"noise\0"
    for frame in range(8):
        digest = hashlib.sha256(
            prefix + frame.to_bytes(8, "little") + (0).to_bytes(4, "little")
        ).digest()
        random = int.from_bytes(digest[:8], "little") >> 11
        noise.append(exact(Fraction(2 * random, 2**53) - 1))

    return {
        "sine-6000": [[sample] for sample in sine_attack],
        "pan-center": center,
        "pan-half": half,
        "one-pole-impulse": [[sample] for sample in one_pole],
        "delay-feedback": [[sample] for sample in feedback],
        "noise-seed7": [[sample] for sample in noise],
    }


def reference_document() -> dict[str, object]:
    references = derive_references()
    return {
        "format": REFERENCE_FORMAT,
        "version": 1,
        "cases": [
            {
                "id": case["id"],
                "frames": [
                    [interval_json(interval) for interval in frame]
                    for frame in references[case["id"]]
                ],
            }
            for case in CASES
        ],
    }


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=True, indent=2) + "\n").encode("utf-8")


def manifest_document(reference_raw: bytes) -> dict[str, object]:
    script_path = Path(__file__)
    return {
        "format": MANIFEST_FORMAT,
        "version": 1,
        "policy": {
            "id": POLICY_ID,
            "sample_rate_hz": "48000",
            "reset_frame": "0",
            "frame_window": ["0", "8"],
            "observed_samples": "finite pre-encoding binary64 decoded exactly as dyadic rationals",
            "reference_samples": "real values enclosed by reduced rational endpoints",
            "metric": "maximum absolute sample error",
            "error_upper": "max_samples(max(abs(actual-lo),abs(actual-hi)))",
            "certification": "pass only when error_upper <= epsilon",
            "epsilon": {
                "numerator": str(EPSILON.numerator),
                "denominator": str(EPSILON.denominator),
            },
            "maximum_interval_width": {
                "numerator": str(MAX_WIDTH.numerator),
                "denominator": str(MAX_WIDTH.denominator),
            },
        },
        "provenance": {
            "derivation": {
                "author": {
                    "identity": "/root/a3_recovery_executor",
                    "role": "challenging executor responsible for independent reference derivation and corpus implementation",
                },
                "generation_environment": {
                    "python_implementation": "CPython",
                    "python_version": "3.14.4",
                    "platform": "macOS 26.6.2 arm64",
                    "standard_library_only": True,
                },
                "script": {
                    "path": "scripts/check_l5_reference.py",
                    "sha256": hash_uri(script_path.read_bytes()),
                },
                "reviewed_scratch_basis": {
                    "path": "target/l5-quantitative-validation/calibration/derive_and_compare.py",
                    "sha256": "sha256:e25339e6a6f465d9fc7072746c53c3d57abaab1f4e24931ff1c6e0420a02b65a",
                    "role": "reviewed calibration derivation adapted into the fixed corpus verifier",
                },
                "arithmetic": "Python stdlib Fraction and hashlib.sha256; no production DSP or libm oracle",
                "square_root": "integer-square inequalities on a 10^-90 rational grid",
                "pi": "Machin 16*atan(1/5)-4*atan(1/239), exact alternating-series remainder brackets through terms 120/121, outward rounded to 100 decimal places",
                "exponential": "exp(-x) exact alternating-series brackets through terms 120/121 at both pi interval endpoints, outward rounded to 80 decimal places",
                "noise": "SHA-256 maac-noise-1 byte preimage mapped to an exact dyadic rational",
            },
            "calibration": {
                "role": "local bound-selection provenance; observed samples are not expected oracles",
                "summary": {
                    "path": "target/l5-quantitative-validation/calibration/summary.json",
                    "sha256": CALIBRATION_SUMMARY_SHA256,
                },
                "environment": "macOS 26.6.2 arm64; rustc 1.95.0; CPython 3.14.4",
                "case_count": "6",
                "channel_sample_count": "64",
                "observed_maximum_error_upper_approx": "1.5935876903e-16",
                "observed_maximum_interval_width": "1/100000000000000000000000000000000000000000000000000000000000000000000000000000000",
                "debug_release_local_sample_bits_equal": True,
                "adoption_rationale": "1/10^14 is above the local enclosure-aware maximum and remains distinct from portable identity or a universal cross-platform claim",
            },
        },
        "reference": {
            "path": REFERENCE_PATH,
            "sha256": hash_uri(reference_raw),
            "bytes": str(len(reference_raw)),
        },
        "assets": [
            {
                "path": ASSET_PATH,
                "sha256": ASSET_SHA256,
                "bytes": "32",
                "format": "pcm_f32le_interleaved/1",
                "sample_rate_hz": "48000",
                "channels": "1",
                "frames": "8",
            }
        ],
        "cases": [
            {
                "id": case["id"],
                "source": {
                    "path": case["source"],
                    "sha256": case["source_sha256"],
                    "bytes": case["source_bytes"],
                },
                "assets": case["assets"],
                "render": {
                    "sample_rate_hz": "48000",
                    "reset_frame": "0",
                    "frame_start": "0",
                    "frame_end": "8",
                    "score_frames": case["score_frames"],
                    "tail_frames": case["tail_frames"],
                    "output_port": case["output_port"],
                    "channels": case["channels"],
                    "channel_order": case["channel_order"],
                },
                "timing": case["timing"],
            }
            for case in CASES
        ],
        "claims": {
            "static_reference_verified": True,
            "runtime_render_executed_by_this_checker": False,
            "cross_platform_bound_proven": False,
            "full_core_audio_profile_covered": False,
        },
    }


def reject_duplicate(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def reject_float(value: str) -> NoReturn:
    fail(f"JSON floating-point number is forbidden: {value}")


def load_strict_json(raw: bytes, label: str) -> object:
    if raw.startswith(b"\xef\xbb\xbf"):
        fail(f"{label}: UTF-8 BOM is forbidden")
    try:
        text = raw.decode("utf-8", errors="strict")
        return json.loads(
            text,
            object_pairs_hook=reject_duplicate,
            parse_float=reject_float,
            parse_constant=reject_float,
        )
    except CorpusError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{label}: invalid JSON: {error}")


def safe_read(corpus: Path, relative: str) -> bytes:
    path = corpus / relative
    if path.is_symlink():
        fail(f"symlink is forbidden: {relative}")
    try:
        resolved = path.resolve(strict=True)
    except FileNotFoundError:
        fail(f"missing required file: {relative}")
    try:
        resolved.relative_to(corpus)
    except ValueError:
        fail(f"path escapes corpus: {relative}")
    if not resolved.is_file():
        fail(f"not a regular file: {relative}")
    return resolved.read_bytes()


def reference_write_path(corpus_argument: Path) -> Path:
    if corpus_argument.is_symlink():
        fail("corpus root must not be a symlink")
    try:
        corpus = corpus_argument.resolve(strict=True)
    except FileNotFoundError:
        fail("corpus root does not exist")
    except OSError as error:
        fail(f"cannot resolve corpus root: {error}")
    if not corpus.is_dir():
        fail("corpus root is not a directory")
    parent = corpus / Path(REFERENCE_PATH).parent
    if parent.is_symlink():
        fail("reference directory must not be a symlink")
    try:
        resolved_parent = parent.resolve(strict=True)
    except FileNotFoundError:
        fail("reference directory does not exist")
    except OSError as error:
        fail(f"cannot resolve reference directory: {error}")
    if not resolved_parent.is_dir():
        fail("reference directory is not a directory")
    try:
        resolved_parent.relative_to(corpus)
    except ValueError:
        fail("reference directory escapes corpus")
    path = resolved_parent / Path(REFERENCE_PATH).name
    if path.is_symlink():
        fail("reference output must not be a symlink")
    if path.exists() and not path.is_file():
        fail("reference output is not a regular file")
    return path


def verify(corpus_argument: Path) -> None:
    if corpus_argument.is_symlink():
        fail("corpus root must not be a symlink")
    corpus = corpus_argument.resolve(strict=True)
    if not corpus.is_dir():
        fail("corpus root is not a directory")

    expected_paths = {
        "README.md",
        "manifest.json",
        REFERENCE_PATH,
        ASSET_PATH,
        *(case["source"] for case in CASES),
    }
    actual_paths: set[str] = set()
    for path in corpus.rglob("*"):
        relative = path.relative_to(corpus).as_posix()
        if path.is_symlink():
            fail(f"symlink is forbidden: {relative}")
        if path.is_file():
            actual_paths.add(relative)
    if actual_paths != expected_paths:
        missing = sorted(expected_paths - actual_paths)
        extra = sorted(actual_paths - expected_paths)
        fail(f"corpus inventory mismatch: missing={missing}, extra={extra}")

    reference_expected = json_bytes(reference_document())
    reference_actual = safe_read(corpus, REFERENCE_PATH)
    load_strict_json(reference_actual, REFERENCE_PATH)
    if reference_actual != reference_expected:
        fail("reference bytes do not match independent derivation")

    references = derive_references()
    sample_count = 0
    maximum_width = Fraction(0)
    for case in CASES:
        frames = references[case["id"]]
        if len(frames) != 8 or any(len(frame) != int(case["channels"]) for frame in frames):
            fail(f"{case['id']}: reference shape mismatch")
        for frame in frames:
            for lo, hi in frame:
                if lo > hi:
                    fail(f"{case['id']}: reversed reference interval")
                maximum_width = max(maximum_width, hi - lo)
                sample_count += 1
    if sample_count != 64:
        fail(f"reference sample count is {sample_count}, expected 64")
    if maximum_width > MAX_WIDTH:
        fail(f"reference interval width {maximum_width} exceeds {MAX_WIDTH}")

    expected_impulse = struct.pack("<f", 1.0) + struct.pack("<f", 0.0) * 7
    asset_actual = safe_read(corpus, ASSET_PATH)
    if asset_actual != expected_impulse or hash_uri(asset_actual) != ASSET_SHA256:
        fail("impulse asset bytes do not match the fixed 1,0,...,0 binary32 vector")

    for case in CASES:
        source_raw = safe_read(corpus, case["source"])
        if len(source_raw) != int(case["source_bytes"]):
            fail(f"{case['id']}: source byte count mismatch")
        if hash_uri(source_raw) != case["source_sha256"]:
            fail(f"{case['id']}: source hash does not match the fixed source oracle")
    one_pole_source = safe_read(corpus, "sources/one-pole-impulse.maac")
    if ASSET_SHA256.encode("ascii") not in one_pole_source or b"ASSET_HASH" in one_pole_source:
        fail("one-pole source does not contain the literal fixed impulse hash")

    manifest_expected = json_bytes(manifest_document(reference_expected))
    manifest_actual = safe_read(corpus, "manifest.json")
    load_strict_json(manifest_actual, "manifest.json")
    if manifest_actual != manifest_expected:
        fail("manifest bytes do not match the fixed corpus contract")

    print(
        "PASS L5 static reference corpus: "
        f"cases={len(CASES)} frames={len(CASES) * 8} samples={sample_count} "
        f"max_interval_width<={fraction_text(MAX_WIDTH)} runtime_render=false"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=Path("conformance/l5"))
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--verify", action="store_true", help="read-only fixed corpus verification")
    action.add_argument("--write-reference", action="store_true", help="write only the derived reference file")
    args = parser.parse_args()
    try:
        if args.write_reference:
            path = reference_write_path(args.corpus)
            raw = json_bytes(reference_document())
            try:
                path.write_bytes(raw)
            except OSError as error:
                fail(f"cannot write reference output: {error}")
            print(f"WROTE {path}: bytes={len(raw)} sha256={hash_uri(raw)}")
        else:
            verify(args.corpus)
        return 0
    except (CorpusError, FileNotFoundError) as error:
        print(f"FAIL L5 reference corpus: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
