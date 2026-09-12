#!/usr/bin/env python3
"""Focused stdlib regressions for the fixed L1 corpus checker."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

from check_identity_edit import canonical_bytes


ROOT = Path(__file__).resolve().parents[1]
SOURCE_CORPUS = ROOT / "conformance" / "l1"
CHECKER = ROOT / "scripts" / "check_identity_edit.py"


class CheckerCorpusFailures(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.corpus = Path(self.temporary.name) / "l1"
        shutil.copytree(SOURCE_CORPUS, self.corpus)

    def manifest(self) -> dict[str, Any]:
        return json.loads((self.corpus / "manifest.json").read_text(encoding="utf-8"))

    def write_manifest(self, manifest: dict[str, Any]) -> None:
        (self.corpus / "manifest.json").write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )

    def write_fixture(self, relative: str, value: Any, manifest: dict[str, Any]) -> None:
        raw = canonical_bytes(value)
        (self.corpus / relative).write_bytes(raw)
        manifest["files"][relative]["bytes"] = len(raw)
        manifest["files"][relative]["sha256"] = hashlib.sha256(raw).hexdigest()

    def run_checker(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(CHECKER), "--corpus", str(self.corpus)],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )

    def assert_clean_rejection(self, expected: str) -> None:
        result = self.run_checker()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stderr, "")
        self.assertIn("FAIL L1 identity corpus:", result.stdout)
        self.assertIn(expected, result.stdout)
        self.assertNotIn("Traceback", result.stdout + result.stderr)

    def test_valid_corpus_passes(self) -> None:
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS L1 identity corpus:", result.stdout)
        self.assertIn("cases=20", result.stdout)
        self.assertIn("protocol=2", result.stdout)
        self.assertIn("revision_authority=maac.revision.authored.sha256/1", result.stdout)
        self.assertIn("normalizer-conformance=false; editor-conformance=false", result.stdout)

    def test_authored_presence_cannot_be_collapsed_into_normalized_identity(self) -> None:
        manifest = self.manifest()
        relative = "authored/minimal-tail-omitted.typed.json"
        value = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        value["objects"]["p"]["fields"]["tail"] = {
            "d": "1",
            "n": "0",
            "t": "quantity",
            "u": "s",
        }
        self.write_fixture(relative, value, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("authored revisions must be distinct")

    def test_revision_authority_is_bound_to_adopted_algorithm(self) -> None:
        manifest = self.manifest()
        manifest["authority"]["revision_algorithm"] = "maac.revision.normalized.sha256/1"
        self.write_manifest(manifest)
        self.assert_clean_rejection("adopted authored revision/protocol contract mismatch")

    def test_patch_case_relationships_cannot_swap_expected_after(self) -> None:
        manifest = self.manifest()
        case = next(item for item in manifest["patch_cases"] if item["id"] == "set_tail_zero_and_inverse")
        case["expected_after"] = "authored/minimal-tail-one.typed.json"
        self.write_manifest(manifest)
        self.assert_clean_rejection("success snapshot relationships mismatch")

    def test_rename_payload_corruption_fails_even_with_updated_digest(self) -> None:
        manifest = self.manifest()
        relative = "patches/rename-tempo-and-set.request.json"
        request = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        request["operations"][1]["value"]["path"] = ["clock"]
        self.write_fixture(relative, request, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("fixed request SHA-256 mismatch")

    def test_recursive_label_erasure_fails_projection(self) -> None:
        manifest = self.manifest()
        relative = "fragments/metadata-role-expected.typed-object.json"
        value = json.loads((self.corpus / relative).read_text(encoding="utf-8"))

        def erase_every_label(current: Any) -> Any:
            if isinstance(current, dict):
                return {key: erase_every_label(item) for key, item in current.items() if key != "label"}
            if isinstance(current, list):
                return [erase_every_label(item) for item in current]
            return current

        self.write_fixture(relative, erase_every_label(value), manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("projection mismatch")

    def test_non_string_quantity_unit_fails_cleanly(self) -> None:
        manifest = self.manifest()
        relative = "authored/minimal-tail-zero.typed.json"
        value = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        value["objects"]["p"]["fields"]["tail"]["u"] = []
        self.write_fixture(relative, value, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("unsupported typed-tree unit")

    def test_non_object_authority_fails_cleanly(self) -> None:
        manifest = self.manifest()
        manifest["authority"] = []
        self.write_manifest(manifest)
        self.assert_clean_rejection("manifest.authority")

    def test_non_string_normalization_reference_fails_cleanly(self) -> None:
        manifest = self.manifest()
        manifest["normalization_vectors"][0]["members"][0]["authored"] = []
        self.write_manifest(manifest)
        self.assert_clean_rejection("normalization vector tail_default_presence authored")

    def test_non_string_patch_case_reference_fails_cleanly(self) -> None:
        manifest = self.manifest()
        manifest["patch_cases"][0]["current"] = []
        self.write_manifest(manifest)
        self.assert_clean_rejection("current does not match fixed contract")

    def test_unknown_patch_operation_field_fails_cleanly(self) -> None:
        manifest = self.manifest()
        relative = "patches/set-tail-zero.request.json"
        request = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        request["operations"][0]["unexpected"] = True
        self.write_fixture(relative, request, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("invalid set fields")

    def test_shortened_fixed_case_operations_fail_cleanly(self) -> None:
        manifest = self.manifest()
        relative = "patches/rename-tempo-and-set.request.json"
        request = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        request["operations"] = request["operations"][:1]
        self.write_fixture(relative, request, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("fixed request SHA-256 mismatch")

    def test_missing_named_normalized_object_fails_cleanly(self) -> None:
        manifest = self.manifest()
        relative = "normalized/minimal-default.execution.json"
        value = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        del value["objects"]["p"]
        self.write_fixture(relative, value, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("unexpected object set")

    def test_missing_fixed_fixture_fails_cleanly(self) -> None:
        manifest = self.manifest()
        relative = "authored/minimal-tail-omitted.typed.json"
        (self.corpus / relative).unlink()
        del manifest["files"][relative]
        self.write_manifest(manifest)
        self.assert_clean_rejection("missing required fixed files")

    def test_changed_forward_value_fails_even_with_updated_manifest_digest(self) -> None:
        manifest = self.manifest()
        relative = "patches/set-tail-zero.request.json"
        request = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        request["operations"][0]["value"]["n"] = "1"
        self.write_fixture(relative, request, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("fixed request SHA-256 mismatch")

    def test_changed_inverse_fails_even_with_updated_manifest_digest(self) -> None:
        manifest = self.manifest()
        relative = "patches/unset-tail-zero.inverse.request.json"
        request = json.loads((self.corpus / relative).read_text(encoding="utf-8"))
        request["operations"][0]["field"] = ["seed"]
        self.write_fixture(relative, request, manifest)
        self.write_manifest(manifest)
        self.assert_clean_rejection("fixed request SHA-256 mismatch")

    def test_boolean_authority_version_does_not_equal_integer_one(self) -> None:
        manifest = self.manifest()
        manifest["authority"]["language_version"] = True
        self.write_manifest(manifest)
        self.assert_clean_rejection("language_version: expected an integer")

    def test_float_authority_version_does_not_equal_integer_one(self) -> None:
        manifest = self.manifest()
        manifest["authority"]["syntax_tree_version"] = 1.0
        self.write_manifest(manifest)
        self.assert_clean_rejection("syntax_tree_version: expected an integer")


if __name__ == "__main__":
    unittest.main(verbosity=2)
