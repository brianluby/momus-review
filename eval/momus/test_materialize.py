#!/usr/bin/env python3
"""Verify that Git review inputs retain exact source and exclude evaluation labels."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
import os
import materialize as tool

from materialize import materialize

ROOT = Path(__file__).resolve().parent


class MaterializeTests(unittest.TestCase):
    def test_copy_mutation_cannot_receive_the_expected_source_receipt(self):
        manifest = json.loads((ROOT / "manifest.json").read_text())
        case = next(c for c in manifest["cases"] if c["expectation"] == "defect")
        original = tool.populate

        def mutated_copy(source, target):
            original(source, target)
            (target / "unexpected-source.txt").write_text("changed after validation")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with mock.patch.object(tool, "populate", side_effect=mutated_copy):
                with self.assertRaisesRegex(ValueError, "Copied Git source"):
                    materialize(ROOT / "manifest.json", case["id"], root / "input", root / "receipt.json")
            self.assertFalse((root / "receipt.json").exists())
            self.assertFalse((root / "input").exists())

    def test_exact_git_objects_and_stable_identity_without_labels(self):
        manifest = json.loads((ROOT / "manifest.json").read_text())
        case = next(c for c in manifest["cases"] if c["expectation"] == "defect")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            first = materialize(ROOT / "manifest.json", case["id"], root / "first", root / "first.json")
            second = materialize(ROOT / "manifest.json", case["id"], root / "second", root / "second.json")
            self.assertEqual(first, second)
            self.assertNotEqual(first["reviewedBase"], first["reviewedHead"])
            for kind, commit in [("base", first["reviewedBase"]), ("head", first["reviewedHead"])]:
                expected = ROOT / case[kind]
                paths = subprocess.check_output(["git", "ls-tree", "-r", "--name-only", commit], cwd=root / "first").decode().splitlines()
                self.assertEqual(paths, sorted(p.relative_to(expected).as_posix() for p in expected.rglob("*") if p.is_file()))
                self.assertNotIn("manifest.json", paths)
                self.assertNotIn("first.json", paths)
                for path in paths:
                    data = subprocess.check_output(["git", "show", commit + ":" + path], cwd=root / "first")
                    self.assertEqual(hashlib.sha256(data).hexdigest(), hashlib.sha256((expected / path).read_bytes()).hexdigest())
            self.assertEqual(subprocess.check_output(["git", "status", "--porcelain"], cwd=root / "first"), b"")

    def test_inherited_git_overrides_do_not_mutate_unrelated_checkout(self):
        case_id = json.loads((ROOT / "manifest.json").read_text())["cases"][0]["id"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            unrelated = root / "unrelated"
            unrelated.mkdir()
            subprocess.run(["git", "init", "--initial-branch=unrelated"], cwd=unrelated,
                           check=True, capture_output=True)
            original = (unrelated / ".git" / "HEAD").read_bytes()
            with mock.patch.dict(os.environ, {"GIT_DIR": str(unrelated / ".git"),
                 "GIT_WORK_TREE": str(unrelated), "GIT_INDEX_FILE": str(root / "unrelated-index"),
                 "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.bare", "GIT_CONFIG_VALUE_0": "true"}):
                result = materialize(ROOT / "manifest.json", case_id, root / "input", root / "receipt.json")
            self.assertEqual((unrelated / ".git" / "HEAD").read_bytes(), original)
            self.assertFalse((unrelated / ".git" / "refs" / "heads" / "unrelated").exists())
            self.assertFalse((root / "unrelated-index").exists())
            self.assertEqual(subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root / "input").decode().strip(), result["reviewedHead"])

    def test_receipt_write_failure_removes_only_our_partial_receipt(self):
        case_id = json.loads((ROOT / "manifest.json").read_text())["cases"][0]["id"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with mock.patch.object(tool.json, "dumps", side_effect=OSError("simulated disk failure")):
                with self.assertRaises(OSError):
                    materialize(ROOT / "manifest.json", case_id, root / "input", root / "receipt.json")
            self.assertFalse((root / "receipt.json").exists())
            self.assertFalse((root / "input").exists())

    def test_existing_outputs_and_label_paths_are_preserved(self):
        case_id = json.loads((ROOT / "manifest.json").read_text())["cases"][0]["id"]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "existing"
            output.mkdir()
            sentinel = output / "unrelated"
            sentinel.write_text("preserve")
            with self.assertRaises(ValueError):
                materialize(ROOT / "manifest.json", case_id, output, root / "receipt.json")
            self.assertEqual(sentinel.read_text(), "preserve")
            self.assertFalse((root / "receipt.json").exists())
            with self.assertRaises(ValueError):
                materialize(ROOT / "manifest.json", case_id, root / "input", root / "input" / "receipt.json")
            with self.assertRaises(ValueError):
                materialize(ROOT / "manifest.json", case_id, ROOT / "accidental-input", root / "receipt.json")


if __name__ == "__main__":
    unittest.main()
