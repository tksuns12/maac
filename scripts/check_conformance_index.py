#!/usr/bin/env python3
"""Validate the static conformance evidence index without running suite recipes.

The index binds the seven native conformance manifests and every source, vector,
asset, or contract payload they reference.  This checker only proves
the index's local path, schema, and byte-digest relationships.  It never
executes the commands recorded in ``check.argv`` and makes no runtime or
profile claim for any suite.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path, PurePosixPath
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_INDEX = Path("conformance/index.json")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
RAW_DIGEST = re.compile(r"[0-9a-f]{64}\Z")
URL_SCHEME = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*:")
ASSET_BLOCK = re.compile(rb"\basset\s+[A-Za-z_][A-Za-z0-9_]*\s*\{(?P<body>.*?)\}", re.DOTALL)
ASSET_PATH = re.compile(rb"\bpath\s*=\s*\"([^\"\\]*)\"")

SUITE_IDS = ["l1", "l2", "l3-inputs", "l3-pan", "l3-tuning", "l4", "l5"]
L5_CASE_IDS = {
    "sine-6000",
    "pan-center",
    "pan-half",
    "one-pole-impulse",
    "delay-feedback",
    "noise-seed7",
}
SUITES: dict[str, dict[str, Any]] = {
    "l1": {
        "manifest_path": "conformance/l1/manifest.json",
        "schema": "maac.conformance.identity-edit/2",
        "coverage": "authored identity, normalization, editing, and projection vectors",
        "check_kind": "static_corpus",
        "argv": ["python3", "scripts/check_identity_edit.py", "--corpus", "conformance/l1"],
    },
    "l2": {
        "manifest_path": "conformance/l2/expected.json",
        "schema": "maac.l2.timing-corpus/1",
        "coverage": "score and seconds timing, reset origin, gate, and retained audio vectors",
        "check_kind": "runtime_suite",
        "argv": ["cargo", "test", "--test", "l2_timing", "--locked", "--offline"],
    },
    "l3-inputs": {
        "manifest_path": "conformance/l3/inputs/expected.json",
        "schema": "maac.l3.input-contract-corpus/1",
        "coverage": "core input cardinality, zero default, and empty event vectors",
        "check_kind": "runtime_suite",
        "argv": ["cargo", "test", "--test", "l3_contracts", "--locked", "--offline"],
    },
    "l3-pan": {
        "manifest_path": "conformance/l3/expected.json",
        "schema": "maac.l3.pan-contract-corpus/1",
        "coverage": "core.pan raw and effective range and modulation vectors",
        "check_kind": "runtime_suite",
        "argv": ["cargo", "test", "--test", "l3_contracts", "--locked", "--offline"],
    },
    "l3-tuning": {
        "manifest_path": "conformance/l3/tuning/expected.json",
        "schema": "maac.l3.tuning-contract-corpus/1",
        "coverage": "tuning reference field and integer domain vectors",
        "check_kind": "runtime_suite",
        "argv": ["cargo", "test", "--test", "l3_tuning", "--locked", "--offline"],
    },
    "l4": {
        "manifest_path": "conformance/l4/manifest.json",
        "schema": "maac.conformance.generic-interchange/1",
        "coverage": "generic interchange canonical bytes, digests, relationships, and rejection vectors",
        "check_kind": "static_corpus",
        "argv": ["python3", "scripts/check_interchange_lock.py", "--corpus", "conformance/l4"],
    },
    "l5": {
        "manifest_path": "conformance/l5/manifest.json",
        "schema": "maac.l5.core-audio-reference/1",
        "coverage": "audio observation, timing, and validation vectors",
        "check_kind": "runtime_suite",
        "argv": ["cargo", "test", "--locked", "--offline", "--test", "l5_core_audio"],
    },
}


class IndexValidationError(ValueError):
    """A deterministic evidence-index contract failure."""


def fail(message: str) -> None:
    raise IndexValidationError(message)


def exact_keys(value: Any, expected: set[str], context: str) -> None:
    if not isinstance(value, dict):
        fail(f"{context}: expected an object")
    if set(value) != expected:
        fail(f"{context}: expected keys {sorted(expected)}, received {sorted(value)}")


def unique_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            fail(f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def parse_json(raw: bytes, context: str) -> Any:
    if raw.startswith(b"\xef\xbb\xbf"):
        fail(f"{context}: UTF-8 BOM is forbidden")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"{context}: invalid UTF-8: {error}")
    try:
        return json.loads(
            text,
            object_pairs_hook=unique_pairs,
            parse_constant=lambda value: fail(f"{context}: non-finite JSON constant {value}"),
        )
    except IndexValidationError:
        raise
    except (TypeError, ValueError) as error:
        fail(f"{context}: malformed JSON: {error}")


def require_string(value: Any, context: str, *, nonempty: bool = False) -> str:
    if not isinstance(value, str) or (nonempty and not value):
        qualifier = " nonempty" if nonempty else ""
        fail(f"{context}: expected a{qualifier} string")
    return value


def require_digest(value: Any, context: str) -> str:
    if not isinstance(value, str) or DIGEST.fullmatch(value) is None:
        fail(f"{context}: expected sha256: plus 64 lowercase hex digits")
    return value


def native_digest(value: Any, context: str) -> str:
    """Normalize an existing manifest's raw or prefixed digest for comparison."""

    if not isinstance(value, str):
        fail(f"{context}: expected a digest string")
    raw = value[7:] if value.startswith("sha256:") else value
    if RAW_DIGEST.fullmatch(raw) is None:
        fail(f"{context}: expected 64 lowercase hex digits")
    return f"sha256:{raw}"


def require_nonnegative_bytes(value: Any, context: str) -> int:
    if type(value) is not int or value < 0:
        fail(f"{context}: expected a nonnegative integer byte count")
    return value


UNSIGNED_DECIMAL = re.compile(r"(?:0|[1-9][0-9]*)\Z")


def require_unsigned_decimal(value: Any, context: str) -> int:
    if not isinstance(value, str) or UNSIGNED_DECIMAL.fullmatch(value) is None:
        fail(f"{context}: expected an unsigned decimal string")
    return int(value)


def normalized_repo_path(value: Any, context: str) -> PurePosixPath:
    """Validate a normalized repository-relative POSIX path before any read."""

    path = require_string(value, context, nonempty=True)
    if "\x00" in path:
        fail(f"{context}: NUL is forbidden in a path")
    if "\\" in path:
        fail(f"{context}: backslash is forbidden in a repository path")
    if URL_SCHEME.match(path) or "://" in path:
        fail(f"{context}: URL is forbidden in a repository path")
    parsed = PurePosixPath(path)
    if parsed.is_absolute() or path.startswith("/"):
        fail(f"{context}: absolute path is forbidden")
    if any(part in {"", ".", ".."} for part in parsed.parts):
        if ".." in parsed.parts:
            fail(f"{context}: path traversal is forbidden")
        fail(f"{context}: path is not normalized")
    if parsed.as_posix() != path:
        fail(f"{context}: path is not normalized")
    return parsed


def joined_repo_path(base: PurePosixPath, value: Any, context: str) -> PurePosixPath:
    relative = normalized_repo_path(value, context)
    return normalized_repo_path((base / relative).as_posix(), context)


def safe_file(root: Path, relative: PurePosixPath, context: str) -> Path:
    """Constrain every component and reject symlinks before reading the file."""

    root = root.resolve(strict=True)
    candidate = root.joinpath(*relative.parts)
    current = root
    for part in relative.parts:
        current = current / part
        if current.is_symlink():
            fail(f"{context}: symlink is forbidden: {relative.as_posix()}")
    if not candidate.exists():
        fail(f"{context}: file is missing: {relative.as_posix()}")
    if not candidate.is_file():
        fail(f"{context}: expected a regular file: {relative.as_posix()}")
    resolved = candidate.resolve(strict=True)
    if resolved != root and root not in resolved.parents:
        fail(f"{context}: path escapes repository root: {relative.as_posix()}")
    return candidate


def read_file(root: Path, relative: PurePosixPath, context: str) -> bytes:
    return safe_file(root, relative, context).read_bytes()


def file_digest(raw: bytes) -> str:
    return f"sha256:{hashlib.sha256(raw).hexdigest()}"


def source_asset_paths(
    root: Path,
    corpus_base: PurePosixPath,
    source: PurePosixPath,
    context: str,
) -> list[PurePosixPath]:
    raw = read_file(root, source, context)
    blocks = ASSET_BLOCK.findall(raw)
    found: list[PurePosixPath] = []
    for block in blocks:
        matches = ASSET_PATH.findall(block)
        if len(matches) != 1:
            fail(f"{context}: asset declaration must contain exactly one path: {source.as_posix()}")
        try:
            reference = matches[0].decode("utf-8")
        except UnicodeDecodeError as error:
            fail(f"{context}: asset path is not UTF-8: {error}")
        source_parent = PurePosixPath(source.as_posix()).parent
        candidates: list[PurePosixPath] = []
        for candidate_base in (corpus_base, source_parent):
            try:
                candidate = joined_repo_path(candidate_base, reference, f"{context} asset path")
            except IndexValidationError:
                continue
            if candidate not in candidates:
                candidates.append(candidate)
        existing = []
        for candidate in candidates:
            if (root / candidate.as_posix()).exists():
                existing.append(candidate)
        if len(existing) != 1:
            if not existing:
                fail(f"{context}: asset path does not resolve to one file: {reference!r}")
            fail(f"{context}: asset path is ambiguous: {reference!r}")
        safe_file(root, existing[0], f"{context} asset")
        found.append(existing[0])
    return found


def native_asset_reference(
    root: Path,
    corpus_base: PurePosixPath,
    asset: Any,
    context: str,
) -> tuple[PurePosixPath, dict[str, Any]]:
    if not isinstance(asset, dict):
        fail(f"{context}: expected an asset object")
    reference = None
    for key in ("path", "bundle_path", "file"):
        if key in asset:
            if reference is not None and asset[key] != reference:
                fail(f"{context}: conflicting asset paths")
            reference = asset[key]
    if reference is None:
        fail(f"{context}: missing asset path")
    path = joined_repo_path(corpus_base, reference, f"{context}.path")
    metadata: dict[str, Any] = {}
    if "sha256" in asset:
        metadata["sha256"] = native_digest(asset["sha256"], f"{context}.sha256")
    if "bytes" in asset:
        metadata["bytes"] = require_nonnegative_bytes(asset["bytes"], f"{context}.bytes")
    safe_file(root, path, context)
    return path, metadata


def expected_fixture_metadata(
    root: Path,
    suite_id: str,
    manifest_relative: PurePosixPath,
    native: dict[str, Any],
) -> dict[PurePosixPath, dict[str, Any]]:
    corpus_base = manifest_relative.parent
    expected: dict[PurePosixPath, dict[str, Any]] = {}
    if suite_id == "l5":
        exact_keys(
            native,
            {"format", "version", "policy", "provenance", "reference", "assets", "cases", "claims"},
            "l5 native manifest",
        )
        if native["format"] != "maac.l5.core-audio-reference":
            fail("l5 native manifest.format: unsupported format")
        if type(native["version"]) is not int or native["version"] != 1:
            fail("l5 native manifest.version: expected integer 1")

        def add(path: PurePosixPath, metadata: dict[str, Any], context: str) -> None:
            previous = expected.get(path)
            if previous is not None and previous != metadata:
                fail(f"{context}: conflicting metadata for {path.as_posix()}")
            expected[path] = metadata

        reference = native["reference"]
        if not isinstance(reference, dict):
            fail("l5 native reference: expected an object")
        exact_keys(reference, {"path", "sha256", "bytes"}, "l5 native reference")
        reference_path = joined_repo_path(corpus_base, reference["path"], "l5 native reference.path")
        add(
            reference_path,
            {
                "sha256": native_digest(reference["sha256"], "l5 native reference.sha256"),
                "bytes": require_unsigned_decimal(reference["bytes"], "l5 native reference.bytes"),
            },
            "l5 native reference",
        )
        safe_file(root, reference_path, "l5 native reference")

        assets = native["assets"]
        if not isinstance(assets, list):
            fail("l5 native assets: expected an array")
        top_level_assets: set[PurePosixPath] = set()
        for index, asset in enumerate(assets):
            context = f"l5 native assets[{index}]"
            if not isinstance(asset, dict):
                fail(f"{context}: expected an object")
            for field in ("path", "sha256", "bytes"):
                if field not in asset:
                    fail(f"{context}: missing {field}")
            asset_path = joined_repo_path(corpus_base, asset["path"], f"{context}.path")
            metadata = {
                "sha256": native_digest(asset["sha256"], f"{context}.sha256"),
                "bytes": require_unsigned_decimal(asset["bytes"], f"{context}.bytes"),
            }
            if asset_path in top_level_assets:
                fail(f"l5 native assets: duplicate path {asset_path.as_posix()}")
            top_level_assets.add(asset_path)
            add(asset_path, metadata, context)
            safe_file(root, asset_path, context)

        cases = native["cases"]
        if not isinstance(cases, list) or not cases:
            fail("l5 native cases: expected a nonempty array")
        case_ids: set[str] = set()
        for index, case in enumerate(cases):
            context = f"l5 native cases[{index}]"
            if not isinstance(case, dict):
                fail(f"{context}: expected an object")
            case_id = require_string(case.get("id"), f"{context}.id", nonempty=True)
            if case_id in case_ids:
                fail(f"l5 native case IDs: duplicate {case_id!r}")
            case_ids.add(case_id)
            source = case.get("source")
            if not isinstance(source, dict):
                fail(f"{context}.source: expected an object")
            exact_keys(source, {"path", "sha256", "bytes"}, f"{context}.source")
            source_path = joined_repo_path(corpus_base, source["path"], f"{context}.source.path")
            if source_path.suffix != ".maac":
                fail(f"{context}.source.path: expected a .maac source")
            add(
                source_path,
                {
                    "sha256": native_digest(source["sha256"], f"{context}.source.sha256"),
                    "bytes": require_unsigned_decimal(source["bytes"], f"{context}.source.bytes"),
                },
                context,
            )
            safe_file(root, source_path, f"{context}.source")
            case_assets = case.get("assets")
            if not isinstance(case_assets, list):
                fail(f"{context}.assets: expected an array")
            for asset_index, asset_path_value in enumerate(case_assets):
                asset_path = joined_repo_path(
                    corpus_base,
                    asset_path_value,
                    f"{context}.assets[{asset_index}]",
                )
                if asset_path not in top_level_assets:
                    fail(f"{context}.assets[{asset_index}]: not listed in native assets")
                safe_file(root, asset_path, f"{context}.assets[{asset_index}]")
        if case_ids != L5_CASE_IDS:
            fail(f"l5 native case IDs: expected {sorted(L5_CASE_IDS)}, received {sorted(case_ids)}")

        provenance = native["provenance"]
        if not isinstance(provenance, dict):
            fail("l5 native provenance: expected an object")
        derivation = provenance.get("derivation")
        if not isinstance(derivation, dict):
            fail("l5 native provenance.derivation: expected an object")
        script = derivation.get("script")
        if not isinstance(script, dict):
            fail("l5 native provenance.derivation.script: expected an object")
        if set(script) != {"path", "sha256"}:
            fail("l5 native provenance.derivation.script: expected path and sha256")
        script_path = normalized_repo_path(
            script["path"], "l5 native provenance.derivation.script.path"
        )
        if script_path.as_posix() != "scripts/check_l5_reference.py":
            fail("l5 native provenance.derivation.script.path: unexpected derivation script")
        add(
            script_path,
            {"sha256": native_digest(script["sha256"], "l5 native provenance.derivation.script.sha256")},
            "l5 native provenance.derivation.script",
        )
        safe_file(root, script_path, "l5 native provenance.derivation.script")
        return expected

    if suite_id in {"l1", "l4"}:
        files = native.get("files")
        if not isinstance(files, dict) or not files:
            fail(f"{suite_id} native manifest files: expected a nonempty object")
        for relative, metadata in files.items():
            file_relative = joined_repo_path(corpus_base, relative, f"{suite_id} native file path")
            if not isinstance(metadata, dict):
                fail(f"{suite_id} native file {relative}: expected metadata object")
            if "sha256" not in metadata or "bytes" not in metadata:
                fail(f"{suite_id} native file {relative}: missing sha256 or bytes")
            expected[file_relative] = {
                "sha256": native_digest(metadata["sha256"], f"{suite_id} native file {relative}.sha256"),
                "bytes": require_nonnegative_bytes(metadata["bytes"], f"{suite_id} native file {relative}.bytes"),
            }
        return expected

    cases = native.get("cases")
    if not isinstance(cases, list) or not cases:
        fail(f"{suite_id} native manifest cases: expected a nonempty array")
    case_ids: set[str] = set()
    sources: list[PurePosixPath] = []
    for index, case in enumerate(cases):
        context = f"{suite_id} native cases[{index}]"
        if not isinstance(case, dict):
            fail(f"{context}: expected an object")
        case_id = require_string(case.get("id"), f"{context}.id", nonempty=True)
        if case_id in case_ids:
            fail(f"{suite_id} native case IDs: duplicate {case_id!r}")
        case_ids.add(case_id)
        source = joined_repo_path(corpus_base, case.get("source"), f"{context}.source")
        if source.suffix != ".maac":
            fail(f"{context}.source: expected a .maac source")
        safe_file(root, source, f"{context}.source")
        sources.append(source)
        expected[source] = {}

    if "asset" in native:
        path, metadata = native_asset_reference(root, corpus_base, native["asset"], f"{suite_id} native asset")
        expected[path] = metadata

    for source in sources:
        for asset_path in source_asset_paths(root, corpus_base, source, f"{suite_id} source"):
            expected.setdefault(asset_path, {})
    return expected


def index_path(root: Path, supplied: str | None) -> PurePosixPath:
    if supplied is None:
        return DEFAULT_INDEX
    path = Path(supplied)
    if path.is_absolute():
        if ".." in path.parts:
            fail("index path traversal is forbidden")
        if path.is_symlink():
            fail("index path symlink is forbidden")
        root_absolute = root.resolve(strict=True)
        path = path.resolve(strict=False)
        try:
            relative = path.relative_to(root_absolute)
        except ValueError:
            fail("index path escapes repository root")
        return normalized_repo_path(relative.as_posix(), "index path")
    return normalized_repo_path(supplied, "index path")


def validate_index(root: Path, supplied_index: str | None) -> tuple[int, int]:
    relative_index = index_path(root, supplied_index)
    index_raw = read_file(root, relative_index, "index")
    index = parse_json(index_raw, "index")
    exact_keys(index, {"schema", "suites"}, "index")
    if index["schema"] != "maac.conformance.evidence-index/1":
        fail("index.schema: unsupported evidence-index schema")
    suites = index["suites"]
    if not isinstance(suites, list):
        fail("index.suites: expected an array")
    actual_ids: list[str | None] = []
    for index, suite in enumerate(suites):
        if not isinstance(suite, dict):
            actual_ids.append(None)
            continue
        identifier = suite.get("id")
        if not isinstance(identifier, str) or not identifier:
            fail(f"suite IDs: suite[{index}].id must be a nonempty string")
        actual_ids.append(identifier)
    if actual_ids != SUITE_IDS or len(set(actual_ids)) != len(actual_ids):
        fail(f"suite IDs: expected exactly {SUITE_IDS} in this order, received {actual_ids}")

    all_manifest_paths: set[PurePosixPath] = set()
    total_fixtures = 0
    for suite in suites:
        suite_id = suite["id"]
        spec = SUITES[suite_id]
        context = f"suite {suite_id}"
        exact_keys(suite, {"id", "manifest", "fixtures", "coverage", "check_kind", "check"}, context)
        if suite["coverage"] != spec["coverage"]:
            fail(f"{context}.coverage: must remain a classification of native expected content")
        if suite["check_kind"] != spec["check_kind"]:
            fail(f"{context}.check_kind: expected {spec['check_kind']}")

        manifest = suite["manifest"]
        exact_keys(manifest, {"path", "schema", "sha256"}, f"{context}.manifest")
        manifest_relative = normalized_repo_path(manifest["path"], f"{context}.manifest.path")
        if manifest_relative.as_posix() != spec["manifest_path"]:
            fail(f"{context}.manifest.path: unexpected native manifest")
        if manifest_relative in all_manifest_paths:
            fail(f"native manifest IDs: duplicate path {manifest_relative.as_posix()}")
        all_manifest_paths.add(manifest_relative)
        if manifest["schema"] != spec["schema"]:
            fail(f"{context}.manifest.schema: expected {spec['schema']}")
        manifest_digest = require_digest(manifest["sha256"], f"{context}.manifest.sha256")
        manifest_raw = read_file(root, manifest_relative, f"{context}.manifest")
        actual_manifest_digest = file_digest(manifest_raw)
        if manifest_digest != actual_manifest_digest:
            fail(f"{context}: manifest digest mismatch")
        native = parse_json(manifest_raw, f"{context}.native manifest")
        if not isinstance(native, dict):
            fail(f"{context}.native manifest: expected an object")
        if suite_id == "l5":
            if native.get("format") != "maac.l5.core-audio-reference":
                fail(f"{context}: native manifest format mismatch")
            if type(native.get("version")) is not int or native.get("version") != 1:
                fail(f"{context}: native manifest version must be integer 1")
        elif native.get("schema") != spec["schema"]:
            fail(f"{context}: native manifest schema mismatch")

        check = suite["check"]
        exact_keys(check, {"cwd", "argv"}, f"{context}.check")
        if check["cwd"] != ".":
            fail(f"{context}.check.cwd: expected repository root '.'")
        if check["argv"] != spec["argv"] or not isinstance(check["argv"], list):
            fail(f"{context}: check recipe differs from the recorded native command")
        if any(not isinstance(token, str) or not token for token in check["argv"]):
            fail(f"{context}.check.argv: expected nonempty command strings")

        fixtures = suite["fixtures"]
        if not isinstance(fixtures, list):
            fail(f"{context}.fixtures: expected an array")
        indexed: dict[PurePosixPath, str] = {}
        previous: PurePosixPath | None = None
        for index, fixture in enumerate(fixtures):
            fixture_context = f"{context}.fixtures[{index}]"
            exact_keys(fixture, {"path", "sha256"}, fixture_context)
            relative = normalized_repo_path(fixture["path"], f"{fixture_context}.path")
            if relative == manifest_relative:
                fail(f"{fixture_context}: manifest must be pinned separately")
            if relative in indexed:
                fail(f"duplicate fixture path: {relative.as_posix()}")
            if previous is not None and relative.as_posix() <= previous.as_posix():
                fail(f"{context}.fixtures: paths must be sorted and unique")
            previous = relative
            indexed[relative] = require_digest(fixture["sha256"], f"{fixture_context}.sha256")

        native_expected = expected_fixture_metadata(root, suite_id, manifest_relative, native)
        if set(indexed) != set(native_expected):
            missing = sorted(path.as_posix() for path in set(native_expected) - set(indexed))
            extra = sorted(path.as_posix() for path in set(indexed) - set(native_expected))
            fail(f"{context}: fixture inventory mismatch (missing={missing}, extra={extra})")
        for relative, expected_digest in indexed.items():
            metadata = native_expected[relative]
            if "sha256" in metadata and expected_digest != metadata["sha256"]:
                fail(f"{context}: fixture disagrees with native manifest file map: {relative.as_posix()}")
            raw = read_file(root, relative, f"{context}.fixture")
            actual_digest = file_digest(raw)
            if expected_digest != actual_digest:
                fail(f"fixture digest mismatch: {relative.as_posix()}")
            if "bytes" in metadata and len(raw) != metadata["bytes"]:
                fail(f"{context}: native byte count mismatch: {relative.as_posix()}")
        total_fixtures += len(indexed)
    return len(suites), total_fixtures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", default=str(ROOT), help="repository root containing conformance/")
    parser.add_argument("--index", help="repository-relative or rooted path to conformance/index.json")
    args = parser.parse_args()
    try:
        root = Path(args.repo_root).resolve(strict=True)
        suites, fixtures = validate_index(root, args.index)
    except (IndexValidationError, OSError, ValueError) as error:
        print(f"FAIL conformance evidence index: {error}")
        return 1
    print(
        f"PASS conformance evidence index: suites={suites} fixtures={fixtures}; "
        "static path/schema/digest bindings checked; runtime checks NOT RUN"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
