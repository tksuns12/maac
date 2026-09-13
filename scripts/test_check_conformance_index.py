#!/usr/bin/env python3
"""Focused regressions for the static L1--L4 conformance evidence index."""

from __future__ import annotations

import copy
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts" / "check_conformance_index.py"
INDEX_RELATIVE = Path("conformance/index.json")


class ConformanceIndexFailures(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        shutil.copytree(ROOT / "conformance", self.repo / "conformance", symlinks=True)
        (self.repo / "scripts").mkdir()
        shutil.copy2(ROOT / "scripts" / "check_l5_reference.py", self.repo / "scripts" / "check_l5_reference.py")

    @property
    def index_path(self) -> Path:
        return self.repo / INDEX_RELATIVE

    def load_index(self) -> dict[str, Any]:
        return json.loads(self.index_path.read_text(encoding="utf-8"))

    def write_index(self, index: dict[str, Any]) -> None:
        self.index_path.write_text(
            json.dumps(index, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )

    def suite(self, index: dict[str, Any], suite_id: str) -> dict[str, Any]:
        return next(suite for suite in index["suites"] if suite["id"] == suite_id)

    def run_checker(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(CHECKER),
                "--repo-root",
                str(self.repo),
                "--index",
                str(self.index_path),
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )

    def assert_clean_rejection(self, expected: str) -> None:
        result = self.run_checker()
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertNotIn("Traceback", output)
        self.assertIn("FAIL conformance evidence index:", result.stdout)
        self.assertIn(expected, output)
        self.assertEqual(result.stderr, "")

    def test_valid_index_reports_static_scope_without_running_recipes(self) -> None:
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS conformance evidence index:", result.stdout)
        self.assertIn("suites=7", result.stdout)
        self.assertIn("fixtures=179", result.stdout)
        self.assertIn("runtime checks NOT RUN", result.stdout)
        l5 = self.suite(self.load_index(), "l5")
        self.assertEqual(l5["manifest"]["schema"], "maac.l5.core-audio-reference/1")
        self.assertEqual(l5["check_kind"], "runtime_suite")
        self.assertEqual(len(l5["fixtures"]), 9)

    def test_l5_source_byte_mutation_is_rejected_by_unchanged_index(self) -> None:
        relative = self.repo / "conformance/l5/sources/sine-6000.maac"
        raw = bytearray(relative.read_bytes())
        raw[0] ^= 1
        relative.write_bytes(raw)
        self.assert_clean_rejection("fixture digest mismatch")

    def test_l5_reference_byte_mutation_is_rejected_by_unchanged_index(self) -> None:
        relative = self.repo / "conformance/l5/references/sample-intervals.json"
        raw = bytearray(relative.read_bytes())
        raw[0] ^= 1
        relative.write_bytes(raw)
        self.assert_clean_rejection("fixture digest mismatch")

    def test_l5_asset_byte_mutation_is_rejected_by_unchanged_index(self) -> None:
        relative = self.repo / "conformance/l5/assets/impulse.pcm"
        raw = bytearray(relative.read_bytes())
        raw[0] ^= 1
        relative.write_bytes(raw)
        self.assert_clean_rejection("fixture digest mismatch")

    def test_l5_derivation_script_byte_mutation_is_rejected_by_unchanged_index(self) -> None:
        relative = self.repo / "scripts/check_l5_reference.py"
        raw = bytearray(relative.read_bytes())
        raw[0] ^= 1
        relative.write_bytes(raw)
        self.assert_clean_rejection("fixture digest mismatch")

    def test_l5_manifest_version_requires_strict_integer_one(self) -> None:
        manifest_path = self.repo / "conformance/l5/manifest.json"
        original = json.loads(manifest_path.read_text(encoding="utf-8"))
        for value in (True, 1.0, "1"):
            with self.subTest(version=value):
                manifest = copy.deepcopy(original)
                manifest["version"] = value
                raw = json.dumps(manifest, ensure_ascii=True, indent=2).encode("utf-8") + b"\n"
                manifest_path.write_bytes(raw)
                index = self.load_index()
                self.suite(index, "l5")["manifest"]["sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
                self.write_index(index)
                self.assert_clean_rejection("native manifest version")

    def test_l5_manifest_rejects_generic_files_map(self) -> None:
        manifest_path = self.repo / "conformance/l5/manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["files"] = {}
        raw = json.dumps(manifest, ensure_ascii=True, indent=2).encode("utf-8") + b"\n"
        manifest_path.write_bytes(raw)
        index = self.load_index()
        self.suite(index, "l5")["manifest"]["sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.write_index(index)
        self.assert_clean_rejection("l5 native manifest")

    def test_l5_native_case_identity_is_closed(self) -> None:
        manifest_path = self.repo / "conformance/l5/manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["cases"][0]["id"] = "renamed-case"
        raw = json.dumps(manifest, ensure_ascii=True, indent=2).encode("utf-8") + b"\n"
        manifest_path.write_bytes(raw)
        index = self.load_index()
        self.suite(index, "l5")["manifest"]["sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.write_index(index)
        self.assert_clean_rejection("l5 native case IDs")

    def test_source_byte_mutation_is_rejected_by_unchanged_index(self) -> None:
        relative = self.repo / "conformance/l2/automation/score.maac"
        raw = bytearray(relative.read_bytes())
        raw[0] ^= 1
        relative.write_bytes(raw)
        self.assert_clean_rejection("fixture digest mismatch")

    def test_missing_fixture_is_rejected(self) -> None:
        index = self.load_index()
        suite = self.suite(index, "l2")
        suite["fixtures"].pop()
        self.write_index(index)
        self.assert_clean_rejection("fixture inventory")

    def test_duplicate_fixture_is_rejected(self) -> None:
        index = self.load_index()
        suite = self.suite(index, "l3-tuning")
        suite["fixtures"].append(copy.deepcopy(suite["fixtures"][0]))
        self.write_index(index)
        self.assert_clean_rejection("duplicate fixture path")

    def test_duplicate_suite_is_rejected(self) -> None:
        index = self.load_index()
        index["suites"].append(copy.deepcopy(index["suites"][-1]))
        self.write_index(index)
        self.assert_clean_rejection("suite IDs")

    def test_non_string_suite_id_is_rejected_cleanly(self) -> None:
        index = self.load_index()
        self.suite(index, "l2")["id"] = []
        self.write_index(index)
        result = self.run_checker()
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertEqual(result.stderr, "")
        self.assertNotIn("Traceback", output)
        self.assertIn("suite[1].id must be a nonempty string", output)

    def test_missing_native_suite_is_rejected(self) -> None:
        index = self.load_index()
        index["suites"] = [suite for suite in index["suites"] if suite["id"] != "l4"]
        self.write_index(index)
        self.assert_clean_rejection("suite IDs")

    def test_manifest_schema_binding_is_rejected(self) -> None:
        index = self.load_index()
        self.suite(index, "l3-pan")["manifest"]["schema"] = "maac.l3.tuning-contract-corpus/1"
        self.write_index(index)
        self.assert_clean_rejection("manifest.schema")

    def test_manifest_digest_mismatch_is_rejected(self) -> None:
        index = self.load_index()
        self.suite(index, "l1")["manifest"]["sha256"] = "sha256:" + "0" * 64
        self.write_index(index)
        self.assert_clean_rejection("manifest digest mismatch")

    def test_path_traversal_is_rejected_before_reads(self) -> None:
        index = self.load_index()
        self.suite(index, "l2")["fixtures"][0]["path"] = "conformance/l2/../l1/manifest.json"
        self.write_index(index)
        self.assert_clean_rejection("path traversal")

    def test_absolute_path_is_rejected_before_reads(self) -> None:
        index = self.load_index()
        self.suite(index, "l2")["fixtures"][0]["path"] = "/etc/hosts"
        self.write_index(index)
        self.assert_clean_rejection("absolute path")

    def test_url_path_is_rejected_before_reads(self) -> None:
        index = self.load_index()
        self.suite(index, "l2")["fixtures"][0]["path"] = "https://example.invalid/fixture"
        self.write_index(index)
        self.assert_clean_rejection("URL")

    def test_symlink_fixture_is_rejected_before_read(self) -> None:
        relative = self.repo / "conformance/l2/automation/score.maac"
        outside = self.repo / "outside.maac"
        outside.write_bytes(relative.read_bytes())
        relative.unlink()
        relative.symlink_to(outside)
        self.assert_clean_rejection("symlink")

    def test_static_suite_cannot_claim_runtime_coverage(self) -> None:
        index = self.load_index()
        self.suite(index, "l1")["coverage"] = "runtime verified"
        self.write_index(index)
        self.assert_clean_rejection("coverage")

    def test_recorded_recipe_is_validated_but_never_executed(self) -> None:
        index = self.load_index()
        sentinel = self.repo / "recipe-ran"
        self.suite(index, "l1")["check"]["argv"] = [
            sys.executable,
            "-c",
            f"open({str(sentinel)!r}, 'w').close()",
        ]
        self.write_index(index)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertFalse(sentinel.exists(), "index checker executed a recorded recipe")
        self.assertIn("check recipe", result.stdout)


if __name__ == "__main__":
    unittest.main()
