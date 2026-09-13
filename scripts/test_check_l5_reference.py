#!/usr/bin/env python3
"""Focused regressions for the fixed L5 Core Audio reference corpus checker."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any, Callable


ROOT = Path(__file__).resolve().parents[1]
SOURCE_CORPUS = ROOT / "conformance" / "l5"
CHECKER = ROOT / "scripts" / "check_l5_reference.py"
MANIFEST_RELATIVE = Path("manifest.json")
REFERENCE_RELATIVE = Path("references/sample-intervals.json")


class L5ReferenceCorpusFailures(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.workspace = Path(self.temporary.name)
        self.corpus = self.copy_corpus("l5")

    def copy_corpus(self, name: str) -> Path:
        corpus = self.workspace / name
        shutil.copytree(SOURCE_CORPUS, corpus, symlinks=True)
        return corpus

    @staticmethod
    def hash_uri(raw: bytes) -> str:
        return "sha256:" + hashlib.sha256(raw).hexdigest()

    @staticmethod
    def replace_once(raw: bytes, old: bytes, new: bytes) -> bytes:
        if raw.count(old) != 1:
            raise AssertionError(f"expected one occurrence of {old!r}")
        return raw.replace(old, new, 1)

    def load_manifest(self, corpus: Path | None = None) -> dict[str, Any]:
        selected = corpus or self.corpus
        return json.loads(
            (selected / MANIFEST_RELATIVE).read_text(encoding="utf-8")
        )

    def write_manifest(self, manifest: dict[str, Any], corpus: Path | None = None) -> None:
        selected = corpus or self.corpus
        (selected / MANIFEST_RELATIVE).write_text(
            json.dumps(manifest, ensure_ascii=True, indent=2) + "\n",
            encoding="utf-8",
        )

    def refresh_reference_metadata(self, corpus: Path, raw: bytes) -> None:
        manifest = self.load_manifest(corpus)
        manifest["reference"]["sha256"] = self.hash_uri(raw)
        manifest["reference"]["bytes"] = str(len(raw))
        self.write_manifest(manifest, corpus)

    def refresh_source_metadata(
        self, corpus: Path, relative: Path, raw: bytes
    ) -> None:
        manifest = self.load_manifest(corpus)
        matching = [
            case
            for case in manifest["cases"]
            if case["source"]["path"] == relative.as_posix()
        ]
        self.assertEqual(len(matching), 1)
        matching[0]["source"]["sha256"] = self.hash_uri(raw)
        matching[0]["source"]["bytes"] = str(len(raw))
        self.write_manifest(manifest, corpus)

    def snapshot_files(self, corpus: Path | None = None) -> dict[str, bytes]:
        selected = corpus or self.corpus
        return {
            path.relative_to(selected).as_posix(): path.read_bytes()
            for path in selected.rglob("*")
            if path.is_file() and not path.is_symlink()
        }

    def run_checker(
        self,
        corpus: Path | None = None,
        action: str = "--verify",
    ) -> subprocess.CompletedProcess[str]:
        selected = corpus or self.corpus
        return subprocess.run(
            [sys.executable, str(CHECKER), "--corpus", str(selected), action],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )

    def assert_clean_rejection(
        self,
        result: subprocess.CompletedProcess[str],
        expected: str,
    ) -> None:
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertIn("FAIL L5 reference corpus:", output)
        self.assertIn(expected, output)
        self.assertNotIn("Traceback", output)

    def test_valid_corpus_is_read_only_and_reports_runtime_false(self) -> None:
        before = self.snapshot_files()
        result = self.run_checker()
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("PASS L5 static reference corpus:", output)
        self.assertIn("cases=6", output)
        self.assertIn("runtime_render=false", output)
        self.assertEqual(before, self.snapshot_files())

    def test_reference_mutation_fails_independent_derivation_after_hash_refresh(self) -> None:
        reference_path = self.corpus / REFERENCE_RELATIVE
        original = reference_path.read_bytes()
        self.assertIn(b'"lo": "0/1"', original)
        raw = original.replace(b'"lo": "0/1"', b'"lo": "1/1"', 1)
        self.assertEqual(len(raw), reference_path.stat().st_size)
        reference_path.write_bytes(raw)
        self.refresh_reference_metadata(self.corpus, raw)
        result = self.run_checker()
        self.assert_clean_rejection(
            result, "reference bytes do not match independent derivation"
        )

    def test_fixed_source_oracle_rejects_same_length_mutation_after_hash_refresh(self) -> None:
        relative = Path("sources/sine-6000.maac")
        source_path = self.corpus / relative
        raw = self.replace_once(
            source_path.read_bytes(), b"pitch = 6000Hz;", b"pitch = 6001Hz;"
        )
        self.assertEqual(len(raw), source_path.stat().st_size)
        source_path.write_bytes(raw)
        self.refresh_source_metadata(self.corpus, relative, raw)
        result = self.run_checker()
        self.assert_clean_rejection(
            result, "source hash does not match the fixed source oracle"
        )

    def test_missing_file_fails_inventory_before_reads(self) -> None:
        (self.corpus / REFERENCE_RELATIVE).unlink()
        self.assert_clean_rejection(
            self.run_checker(), "corpus inventory mismatch"
        )

    def test_extra_file_fails_inventory_before_reads(self) -> None:
        (self.corpus / "unexpected.bin").write_bytes(b"unexpected")
        self.assert_clean_rejection(
            self.run_checker(), "corpus inventory mismatch"
        )

    def test_verify_rejects_corpus_root_symlink(self) -> None:
        linked = self.workspace / "l5-link"
        linked.symlink_to(self.corpus, target_is_directory=True)
        self.assert_clean_rejection(
            self.run_checker(linked), "corpus root must not be a symlink"
        )

    def test_verify_rejects_final_file_symlink_before_following_it(self) -> None:
        reference_path = self.corpus / REFERENCE_RELATIVE
        outside = self.workspace / "outside-reference.json"
        outside.write_bytes(reference_path.read_bytes())
        reference_path.unlink()
        reference_path.symlink_to(outside)
        self.assert_clean_rejection(
            self.run_checker(),
            "symlink is forbidden: references/sample-intervals.json",
        )

    def test_verify_rejects_intermediate_directory_symlink_before_traversal(self) -> None:
        references_path = self.corpus / "references"
        outside = self.workspace / "outside-references"
        shutil.move(references_path, outside)
        references_path.symlink_to(outside, target_is_directory=True)
        self.assert_clean_rejection(
            self.run_checker(), "symlink is forbidden: references"
        )

    def test_strict_manifest_inputs_fail_without_tracebacks(self) -> None:
        def malformed(raw: bytes) -> bytes:
            return b"{\n"

        def duplicate_key(raw: bytes) -> bytes:
            lines = raw.splitlines(keepends=True)
            self.assertGreaterEqual(len(lines), 2)
            return lines[0] + lines[1] + lines[1] + b"".join(lines[2:])

        def bom(raw: bytes) -> bytes:
            return b"\xef\xbb\xbf" + raw

        def nonfinite(raw: bytes) -> bytes:
            return self.replace_once(raw, b'"version": 1,', b'"version": NaN,')

        variants: tuple[tuple[str, Callable[[bytes], bytes], str], ...] = (
            ("malformed", malformed, "manifest.json: invalid JSON"),
            ("duplicate-key", duplicate_key, "duplicate JSON key"),
            ("bom", bom, "manifest.json: UTF-8 BOM is forbidden"),
            ("nonfinite", nonfinite, "JSON floating-point number is forbidden"),
        )
        for name, mutate, expected in variants:
            with self.subTest(name=name):
                corpus = self.copy_corpus(f"strict-{name}")
                manifest_path = corpus / MANIFEST_RELATIVE
                manifest_path.write_bytes(mutate(manifest_path.read_bytes()))
                self.assert_clean_rejection(self.run_checker(corpus), expected)

        for value in (True, 1.0):
            with self.subTest(version=repr(value)):
                corpus = self.copy_corpus(f"version-{repr(value)}")
                manifest = self.load_manifest(corpus)
                manifest["version"] = value
                self.write_manifest(manifest, corpus)
                expected = (
                    "JSON floating-point number is forbidden"
                    if isinstance(value, float)
                    else "manifest"
                )
                self.assert_clean_rejection(
                    self.run_checker(corpus), expected
                )

    def test_recorded_runtime_command_metadata_is_never_executed(self) -> None:
        sentinel = self.workspace / "runtime-command-ran"
        manifest = self.load_manifest()
        manifest["provenance"]["runtime_command"] = [
            sys.executable,
            "-c",
            f"open({str(sentinel)!r}, 'w').write('ran')",
        ]
        self.write_manifest(manifest)
        result = self.run_checker()
        self.assertFalse(sentinel.exists(), "checker executed recorded command")
        self.assert_clean_rejection(result, "manifest")

    def test_write_reference_rejects_corpus_root_symlink(self) -> None:
        linked = self.workspace / "write-root-link"
        linked.symlink_to(self.corpus, target_is_directory=True)
        self.assert_clean_rejection(
            self.run_checker(linked, "--write-reference"),
            "corpus root must not be a symlink",
        )

    def test_write_reference_rejects_reference_directory_or_output_symlink(self) -> None:
        cases: tuple[str, Callable[[Path], None], str] = (
            (
                "directory",
                lambda corpus: self._replace_references_with_symlink(corpus),
                "reference directory must not be a symlink",
            ),
            (
                "output",
                lambda corpus: self._replace_reference_with_symlink(corpus),
                "reference output must not be a symlink",
            ),
        )
        for name, prepare, expected in cases:
            with self.subTest(name=name):
                corpus = self.copy_corpus(f"write-{name}")
                prepare(corpus)
                self.assert_clean_rejection(
                    self.run_checker(corpus, "--write-reference"), expected
                )

    def test_write_reference_rejects_non_directory_reference_paths(self) -> None:
        cases: tuple[str, Callable[[Path], None], str] = (
            (
                "parent-regular-file",
                self._replace_references_with_regular_file,
                "reference directory is not a directory",
            ),
            (
                "output-directory",
                self._replace_reference_with_directory,
                "reference output is not a regular file",
            ),
        )
        for name, prepare, expected in cases:
            with self.subTest(name=name):
                corpus = self.copy_corpus(f"write-nondirectory-{name}")
                prepare(corpus)
                before = self.snapshot_files(corpus)
                self.assert_clean_rejection(
                    self.run_checker(corpus, "--write-reference"), expected
                )
                self.assertEqual(before, self.snapshot_files(corpus))

    def _replace_references_with_symlink(self, corpus: Path) -> None:
        references_path = corpus / "references"
        outside = self.workspace / "write-outside-references"
        shutil.move(references_path, outside)
        references_path.symlink_to(outside, target_is_directory=True)

    def _replace_references_with_regular_file(self, corpus: Path) -> None:
        references_path = corpus / "references"
        backup = self.workspace / f"original-{corpus.name}-references"
        shutil.move(references_path, backup)
        references_path.write_bytes(b"references must be a directory\n")

    def _replace_reference_with_symlink(self, corpus: Path) -> None:
        reference_path = corpus / REFERENCE_RELATIVE
        outside = self.workspace / "write-outside-reference.json"
        outside.write_bytes(reference_path.read_bytes())
        reference_path.unlink()
        reference_path.symlink_to(outside)

    def _replace_reference_with_directory(self, corpus: Path) -> None:
        reference_path = corpus / REFERENCE_RELATIVE
        reference_path.unlink()
        reference_path.mkdir()


if __name__ == "__main__":
    unittest.main()
