#!/usr/bin/env python3
"""Refresh public PR #40 development controls from immutable local Git objects.

These exposed examples are never held-out benchmark evidence. The original
files are copied verbatim; surrounding README contract context is authored.
"""
import difflib
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
HEAD = "4bbcd16312774109ca2c147989a032c4401ed627"


def original(revision, path):
    return subprocess.check_output(["git", "show", f"{revision}:{path}"], cwd=REPO)


def tree_hash(root):
    values = [{"path": p.relative_to(root).as_posix(),
               "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
              for p in sorted(root.rglob("*")) if p.is_file()]
    return hashlib.sha256(json.dumps(values, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False).encode()).hexdigest()


def build():
    base_rev = subprocess.check_output(["git", "rev-parse", HEAD + "^1"], cwd=REPO).decode().strip()
    license_text = original(HEAD, "LICENSE")
    specifications = [
        ("pr40-utf8-hardening", "src/adapters/git.rs", "reliability",
         "Git source evidence must remain exact UTF-8. Invalid bytes must fail explicitly; "
         "lossy replacement characters cannot prove a source fact. Diagnostics may decode stderr lossily. "
         "Rejecting invalid source bytes is an intentional evidence-integrity contract. "
         "See the unreadable_source_baseline_is_never_reinterpreted_as_an_addition regression test.", False),
        ("pr40-auxiliary-identity", "src/review/auxiliary.rs", "correctness",
         "Source review identity is authoritative. Auxiliary evidence at another head, or without "
         "a captured source head, must preserve reviewedHead and invalidate completeness. "
         "Unknown evidence must prevent automatic approval. This is intentional hardening. "
         "Tests auxiliary_head_cannot_overwrite_verified_source_identity and "
         "auxiliary_head_cannot_fill_missing_source_identity demonstrate both boundaries.", True),
    ]
    cases = []
    for case_id, path, dimension, contract, added in specifications:
        folder = ROOT / "development" / case_id
        for snapshot, revision in [("base", base_rev), ("head", HEAD)]:
            tree = folder / snapshot
            tree.mkdir(parents=True, exist_ok=True)
            (tree / "LICENSE").write_bytes(license_text)
            (tree / "README.md").write_text("# Review evidence contract\n\n" + contract + "\n")
            if snapshot == "head" or not added:
                output = tree / path
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(original(revision, path))
        before = (folder / "base" / path).read_text() if not added else ""
        after = (folder / "head" / path).read_text()
        patch_lines = difflib.unified_diff(before.splitlines(keepends=True),
                                          after.splitlines(keepends=True),
                                          fromfile="a/" + path, tofile="b/" + path)
        patch = "".join(line if line.endswith("\n") else
                        line + "\n\\ No newline at end of file\n" for line in patch_lines)
        (folder / "change.diff").write_text(patch)
        cases.append({
            "id": case_id, "pairId": None, "split": "development", "slice": "core",
            "language": "rust", "expectation": "clean", "dimension": dimension,
            "base": f"development/{case_id}/base", "head": f"development/{case_id}/head",
            "diff": f"development/{case_id}/change.diff",
            "source": {"kind": "observed-control", "license": "MIT",
                       "repository": "https://github.com/brianluby/momus-review", "revision": HEAD,
                       "description": f"Public PR #40 exposed development control. {path} copied verbatim "
                       f"from base {base_rev} and head {HEAD}; README is authored contract context. "
                       "This records an observed intentional change, not an observed defect; bot comments "
                       "are not ground truth. This selected-file context is not a runnable full Momus clone.",
                       "baseSha256": tree_hash(folder / "base"),
                       "headSha256": tree_hash(folder / "head"),
                       "diffSha256": hashlib.sha256(patch.encode()).hexdigest()},
            "issues": [], "abstention": None, "reproducer": None,
            "annotation": {"status": "pending", "reviewer": None, "rationale": contract},
        })
    manifest_path = ROOT / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["cases"] = [c for c in manifest["cases"] if c["id"] not in {v[0] for v in specifications}] + cases
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    print("Refreshed two public development controls; holdout corpus unchanged")


if __name__ == "__main__":
    build()
