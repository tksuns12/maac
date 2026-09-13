#!/usr/bin/env python3
"""Focused regressions for the fixed L4 generic-interchange corpus checker."""

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
from check_interchange_lock import require_unsigned


ROOT = Path(__file__).resolve().parents[1]
SOURCE_CORPUS = ROOT / "conformance" / "l4"
CHECKER = ROOT / "scripts" / "check_interchange_lock.py"


class InterchangeCorpusFailures(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.corpus = Path(self.temporary.name) / "l4"
        shutil.copytree(SOURCE_CORPUS, self.corpus)

    def manifest(self) -> dict[str, Any]:
        return json.loads((self.corpus / "manifest.json").read_text(encoding="utf-8"))

    def _refresh_file(self, relative: str, manifest: dict[str, Any]) -> None:
        raw = (self.corpus / relative).read_bytes()
        manifest["files"][relative]["bytes"] = len(raw)
        manifest["files"][relative]["sha256"] = hashlib.sha256(raw).hexdigest()

    def _write_manifest(self, manifest: dict[str, Any]) -> None:
        (self.corpus / "manifest.json").write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        lines = [
            f"{item['sha256']}  {relative}\n"
            for relative, item in sorted(manifest["files"].items())
        ]
        (self.corpus / "SHA256SUMS").write_text("".join(lines), encoding="ascii")

    def write_json(self, relative: str, value: Any, *, canonical: bool = True) -> None:
        manifest = self.manifest()
        raw = canonical_bytes(value) if canonical else json.dumps(value).encode("utf-8")
        (self.corpus / relative).write_bytes(raw)
        manifest["files"][relative]["canonical"] = canonical
        self._refresh_file(relative, manifest)
        self._write_manifest(manifest)

    def write_bytes(self, relative: str, raw: bytes) -> None:
        manifest = self.manifest()
        (self.corpus / relative).write_bytes(raw)
        self._refresh_file(relative, manifest)
        self._write_manifest(manifest)

    def load_json(self, relative: str) -> Any:
        return json.loads((self.corpus / relative).read_text(encoding="utf-8"))

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
        self.assertIn("FAIL L4 interchange corpus:", result.stdout)
        self.assertIn(expected, result.stdout)
        self.assertNotIn("Traceback", result.stdout + result.stderr)

    def test_valid_corpus_passes_with_bounded_claim(self) -> None:
        result = self.run_checker()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS L4 interchange corpus:", result.stdout)
        self.assertIn("files=54", result.stdout)
        self.assertIn("locks=6", result.stdout)
        self.assertIn("invalids=28", result.stdout)
        self.assertIn("runtime-lock-conformance=false", result.stdout)

    def test_stale_config_digest_fails_after_transport_hash_refresh(self) -> None:
        relative = "canonical/external-ramp-lock.json"
        lock = self.load_json(relative)
        lock["processors"][0]["config"]["config"]["fields"]["amount"]["n"] = "2"
        self.write_json(relative, lock)
        self.assert_clean_rejection("E_DIGEST")

    def test_stale_render_key_fails_after_transport_hash_refresh(self) -> None:
        relative = "canonical/minimal-lock.json"
        lock = self.load_json(relative)
        lock["output"]["crop"][1] = "3"
        self.write_json(relative, lock)
        self.assert_clean_rejection("E_DIGEST")

    def test_cross_pinned_processor_roles_fail_after_transport_hash_refresh(self) -> None:
        relative = "canonical/external-ramp-lock.json"
        lock = self.load_json(relative)
        by_role = {tuple(item["role"]): item for item in lock["dependencies"]}
        adapter = by_role[("processor", "adapter")]
        descriptor = by_role[("processor", "descriptor")]
        adapter["sha256"], descriptor["sha256"] = descriptor["sha256"], adapter["sha256"]
        adapter["bytes"], descriptor["bytes"] = descriptor["bytes"], adapter["bytes"]
        self.write_json(relative, lock)
        self.assert_clean_rejection("E_CLOSURE")

    def test_missing_processor_slot_fails_after_transport_hash_refresh(self) -> None:
        relative = "canonical/external-ramp-lock.json"
        lock = self.load_json(relative)
        lock["dependencies"] = [
            item for item in lock["dependencies"]
            if item["role"] != ["processor", "descriptor"]
        ]
        self.write_json(relative, lock)
        self.assert_clean_rejection("E_CLOSURE")

    def test_pcm_corruption_fails_after_transport_hash_refresh(self) -> None:
        relative = "evidence/minimal.pcm"
        raw = bytearray((self.corpus / relative).read_bytes())
        raw[-1] ^= 1
        self.write_bytes(relative, bytes(raw))
        self.assert_clean_rejection("E_EVIDENCE")

    def test_config_label_erasure_fails_after_transport_hash_refresh(self) -> None:
        relative = "canonical/external-ramp-lock.json"
        lock = self.load_json(relative)
        del lock["processors"][0]["config"]["config"]["fields"]["label"]
        self.write_json(relative, lock)
        self.assert_clean_rejection("E_DIGEST")

    def test_missing_source_asset_fails_after_transport_hash_refresh(self) -> None:
        relative = "preimages/ramp-execution.json"
        execution = self.load_json(relative)
        del execution["objects"]["state_asset"]
        self.write_json(relative, execution)
        self.assert_clean_rejection("E_CLOSURE")

    def test_wrong_source_asset_kind_fails_after_transport_hash_refresh(self) -> None:
        relative = "preimages/ramp-execution.json"
        execution = self.load_json(relative)
        execution["objects"]["state_asset"]["fields"]["kind"]["v"] = "module"
        self.write_json(relative, execution)
        self.assert_clean_rejection("E_CLOSURE")

    def test_source_state_reference_mismatch_fails_after_transport_hash_refresh(self) -> None:
        relative = "preimages/ramp-execution.json"
        execution = self.load_json(relative)
        execution["objects"]["fx"]["fields"]["state"]["path"] = ["impl_asset"]
        self.write_json(relative, execution)
        self.assert_clean_rejection("E_CLOSURE")

    def test_source_implementation_reference_mismatch_fails_after_transport_hash_refresh(self) -> None:
        relative = "preimages/ramp-execution.json"
        execution = self.load_json(relative)
        execution["objects"]["fx"]["fields"]["implementation"]["path"] = ["state_asset"]
        self.write_json(relative, execution)
        self.assert_clean_rejection("E_CLOSURE")

    def test_duplicate_json_key_fails_cleanly(self) -> None:
        relative = "canonical/minimal-lock.json"
        raw = (self.corpus / relative).read_bytes().replace(
            b'"version":1', b'"version":1,"version":1', 1
        )
        self.write_bytes(relative, raw)
        self.assert_clean_rejection("duplicate JSON key")

    def test_utf8_bom_fails_cleanly(self) -> None:
        relative = "canonical/minimal-lock.json"
        raw = b"\xef\xbb\xbf" + (self.corpus / relative).read_bytes()
        self.write_bytes(relative, raw)
        self.assert_clean_rejection("UTF-8 BOM is forbidden")

    def test_escaped_non_ascii_character_is_not_canonical(self) -> None:
        relative = "canonical/external-config.json"
        raw = (self.corpus / relative).read_bytes().replace("雪".encode(), b"\\u96ea", 1)
        self.write_bytes(relative, raw)
        self.assert_clean_rejection("E_CANONICAL")

    def test_final_line_feed_is_not_canonical(self) -> None:
        relative = "canonical/minimal-lock.json"
        self.write_bytes(relative, (self.corpus / relative).read_bytes() + b"\n")
        self.assert_clean_rejection("E_CANONICAL")

    def test_symlinked_corpus_artifact_fails_cleanly(self) -> None:
        relative = "evidence/minimal.pcm"
        fixture = self.corpus / relative
        outside = Path(self.temporary.name) / "outside.pcm"
        outside.write_bytes(fixture.read_bytes())
        fixture.unlink()
        fixture.symlink_to(outside)
        self.assert_clean_rejection("symlink")

    def test_numeric_one_canonical_marker_cannot_bypass_byte_check(self) -> None:
        relative = "canonical/equal-config-negative-lock.json"
        manifest = self.manifest()
        (self.corpus / relative).write_bytes((self.corpus / relative).read_bytes() + b"\n")
        manifest["files"][relative]["canonical"] = 1
        self._refresh_file(relative, manifest)
        self._write_manifest(manifest)
        self.assert_clean_rejection("canonical metadata must be boolean or null")

    def test_numeric_zero_canonical_marker_cannot_bypass_byte_check(self) -> None:
        relative = "canonical/equal-config-negative-lock.json"
        manifest = self.manifest()
        (self.corpus / relative).write_bytes((self.corpus / relative).read_bytes() + b"\n")
        manifest["files"][relative]["canonical"] = 0
        self._refresh_file(relative, manifest)
        self._write_manifest(manifest)
        self.assert_clean_rejection("canonical metadata must be boolean or null")

    def test_symlinked_manifest_fails_before_reading_it(self) -> None:
        fixture = self.corpus / "manifest.json"
        outside = Path(self.temporary.name) / "outside-manifest.json"
        outside.write_bytes(fixture.read_bytes())
        fixture.unlink()
        fixture.symlink_to(outside)
        self.assert_clean_rejection("manifest.json: symlink")

    def test_symlinked_sha256sums_fails_before_reading_it(self) -> None:
        fixture = self.corpus / "SHA256SUMS"
        outside = Path(self.temporary.name) / "outside-SHA256SUMS"
        outside.write_bytes(fixture.read_bytes())
        fixture.unlink()
        fixture.symlink_to(outside)
        self.assert_clean_rejection("SHA256SUMS: symlink")

    def test_unknown_fixture_capability_fails_before_digest_mismatch(self) -> None:
        relative = "preimages/ramp-execution.json"
        execution = self.load_json(relative)
        execution["objects"]["p"]["fields"]["requires"]["items"][0]["v"] = "maac.fixture.unknown/1"
        manifest = self.manifest()
        (self.corpus / relative).write_bytes(canonical_bytes(execution))
        self._refresh_file(relative, manifest)
        relation = next(item for item in manifest["relationships"]["render_inputs"] if item["id"] == "external-ramp")
        relation["source_bindings"]["requires"] = ["maac.fixture.unknown/1"]
        self._write_manifest(manifest)
        self.assert_clean_rejection("E_CAPABILITY")

    def test_large_canonical_unsigned_decimal_is_structurally_accepted(self) -> None:
        value = "9" * 4096
        self.assertEqual(require_unsigned(value, "large"), int(value))

    def test_removed_referenced_fixture_fails_cleanly(self) -> None:
        relative = "contracts/fixture-engine.json"
        manifest = self.manifest()
        (self.corpus / relative).unlink()
        del manifest["files"][relative]
        self._write_manifest(manifest)
        self.assert_clean_rejection("unknown corpus file")


if __name__ == "__main__":
    unittest.main(verbosity=2)
