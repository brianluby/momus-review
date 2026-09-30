#!/usr/bin/env python3
"""Exercise deterministic SBOM identity and the real attestation handoff step."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("finalize_sbom", ROOT / "scripts/finalize-sbom.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
freeze = re.search(
    r"      - name: Freeze checksums\n        run: \|\n(.*?)(?=\n      - name:)",
    (ROOT / ".github/workflows/attest.yml").read_text(), re.S,
)
FREEZE = textwrap.dedent(freeze.group(1))
TARGETS = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin"]


class SbomTests(unittest.TestCase):
    def bom(self):
        return {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
                "components": [{"name": "serde", "version": "1.0.228"}]}

    def test_deterministic_identity_preserves_graph(self):
        original = self.bom()
        result = module.finalize(original)
        self.assertNotIn("serialNumber", original)
        self.assertEqual({k: v for k, v in result.items() if k != "serialNumber"}, original)
        self.assertEqual(result, module.finalize(result))
        self.assertEqual(result, module.finalize(dict(reversed(list(original.items())))))
        changed = self.bom()
        changed["components"][0]["version"] = "2.0.0"
        self.assertNotEqual(result["serialNumber"], module.finalize(changed)["serialNumber"])

    def test_invalid_identity_and_format_rejected(self):
        for bom in [[], {"bomFormat": "SPDX"},
                    {**self.bom(), "serialNumber": "not-a-uuid"},
                    {**self.bom(), "serialNumber": "urn:uuid:invalid"}]:
            with self.subTest(bom=bom), self.assertRaises(ValueError):
                module.finalize(bom)

    def handoff(self, target, mutation=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "dist").mkdir()
            (root / "meta").mkdir()
            name = f"momus-{target}.cdx.json"
            data = json.dumps(module.finalize(self.bom())).encode()
            (root / "meta" / name).write_bytes(data)
            metadata = {"target": target, "sbom": {"name": name,
                        "sha256": hashlib.sha256(data).hexdigest()}}
            (root / "meta/metadata.json").write_text(json.dumps(metadata))
            # Signing replaces this artifact with an archive and checksum
            # only. Both subjects must still survive into final attestations.
            archive = root / "dist" / f"momus-{target}.tar.gz"
            archive.write_bytes(b"final signed archive")
            if mutation:
                mutation(root, name, metadata)
            run = subprocess.run(["bash", "-c", FREEZE], cwd=root,
                                 env={**os.environ, "TARGET": target},
                                 capture_output=True, text=True)
            if not mutation:
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual((root / "dist" / name).read_bytes(), data)
                digest = (root / "dist" / f"momus-{target}.tar.gz.sha256").read_text().split()[0]
                self.assertEqual(digest, hashlib.sha256(archive.read_bytes()).hexdigest())
            return run

    def test_archive_replacement_preserves_sbom_for_every_target(self):
        for target in TARGETS:
            with self.subTest(target=target):
                self.handoff(target)

    def test_missing_sbom_fails_before_attestation(self):
        run = self.handoff(TARGETS[2], lambda root, name, _: (root / "meta" / name).unlink())
        self.assertNotEqual(run.returncode, 0)

    def test_changed_sbom_fails_before_attestation(self):
        run = self.handoff(TARGETS[0], lambda root, name, _: (root / "meta" / name).write_text("{}"))
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("digest does not match", run.stderr)

    def test_wrong_target_fails_before_attestation(self):
        def wrong_target(root, name, metadata):
            metadata["target"] = TARGETS[1]
            (root / "meta/metadata.json").write_text(json.dumps(metadata))
        run = self.handoff(TARGETS[0], wrong_target)
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("target does not match", run.stderr)

    def test_missing_serial_fails_before_attestation(self):
        def no_serial(root, name, metadata):
            data = json.dumps(self.bom()).encode()
            (root / "meta" / name).write_bytes(data)
            metadata["sbom"]["sha256"] = hashlib.sha256(data).hexdigest()
            (root / "meta/metadata.json").write_text(json.dumps(metadata))
        run = self.handoff(TARGETS[0], no_serial)
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("not compatible", run.stderr)


if __name__ == "__main__":
    unittest.main()
